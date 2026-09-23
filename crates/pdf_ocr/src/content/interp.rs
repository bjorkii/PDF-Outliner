//! 그래픽 상태를 추적하는 콘텐츠 스트림 해석기(설계 문서 2.2, 2.3).
//!
//! 텍스트 상태(`Tr`, `Tf` 등)는 그래픽 상태의 일부라 `BT..ET` 밖에서도 설정되고 `ET` 뒤에도
//! 유지된다. 그래서 `BT` 블록 안만 봐서는 `Tr 3`을 놓친다 — 이 해석기는 스트림 전체에서 상태를
//! 이어 간다. Form XObject는 호출한 쪽의 그래픽 상태를 물려받으므로(`Do` 전에 설정한 `Tr 3`
//! 포함) 같은 XObject라도 호출 위치마다 분류가 달라질 수 있다.
//!
//! 해석기 자신은 아무것도 판정하지 않는다. 연산자마다 [`Visitor::operation`]을 그 연산자를
//! 적용하기 **전**의 상태와 함께 부르고, 판정(분류·이미지 영역 계산 등)은 방문자가 한다.
//!
//! 아직 하지 않는 것: 글리프 이동량(폰트 폭)을 반영한 텍스트 행렬 전진. 표시 연산자 뒤의
//! `Tm`은 전진하지 않은 값으로 남는다 — 폭 계산이 필요한 기능(연산자 제거 시 `[n] TJ` 치환,
//! 글자 위치 판정)을 만들 때 폰트 모듈과 함께 넣는다.

use super::lexer::{tokenize, LexError, Operand, Operation};
use crate::geometry::{number, resolve};
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::HashSet;

pub type Matrix = [f64; 6];

pub const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// `a`를 먼저, 그다음 `b`를 적용하는 행렬(PDF 행 벡터 규약: p' = p × a × b).
pub fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    [
        a[0] * b[0] + a[1] * b[2],
        a[0] * b[1] + a[1] * b[3],
        a[2] * b[0] + a[3] * b[2],
        a[2] * b[1] + a[3] * b[3],
        a[4] * b[0] + a[5] * b[2] + b[4],
        a[4] * b[1] + a[5] * b[3] + b[5],
    ]
}

pub fn transform_point(m: &Matrix, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}

pub fn determinant(m: &Matrix) -> f64 {
    m[0] * m[3] - m[1] * m[2]
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextState {
    pub char_spacing: f64,
    pub word_spacing: f64,
    /// `Tz` 값(퍼센트, 기본 100).
    pub horizontal_scaling: f64,
    pub leading: f64,
    /// `Tf`로 지정한 폰트 리소스 이름.
    pub font_name: Option<Vec<u8>>,
    /// ExtGState `/Font`로 지정한 폰트 객체(리소스 이름 없이 직접 참조).
    pub font_ref: Option<ObjectId>,
    pub font_size: f64,
    pub render_mode: i64,
    pub rise: f64,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            char_spacing: 0.0,
            word_spacing: 0.0,
            horizontal_scaling: 100.0,
            leading: 0.0,
            font_name: None,
            font_ref: None,
            font_size: 0.0,
            render_mode: 0,
            rise: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphicsState {
    pub ctm: Matrix,
    pub text: TextState,
    /// ExtGState `ca`(채우기 불투명도).
    pub fill_alpha: f64,
    /// ExtGState `CA`(선 불투명도).
    pub stroke_alpha: f64,
    /// ExtGState `SMask`가 `/None`이 아닌 값으로 설정됨.
    pub soft_mask: bool,
    pub fill_color_space: Vec<u8>,
    pub fill_color: Vec<f64>,
    pub stroke_color_space: Vec<u8>,
    pub stroke_color: Vec<f64>,
    /// 지금까지 적용된 클리핑 영역을 감싸는 사각형(사용자 공간). None이면 제한 없음.
    /// 실제 클리핑 경로가 아니라 경로를 감싸는 사각형이라, "밖에 있다"는 판정에만 쓸 수 있다.
    pub clip: Option<[f64; 4]>,
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self {
            ctm: IDENTITY,
            text: TextState::default(),
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            soft_mask: false,
            fill_color_space: b"DeviceGray".to_vec(),
            fill_color: vec![0.0],
            stroke_color_space: b"DeviceGray".to_vec(),
            stroke_color: vec![0.0],
            clip: None,
        }
    }
}

/// `BDC`/`BMC`로 연 마크드 콘텐츠 한 단계.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkedContent {
    pub tag: Vec<u8>,
    /// 인라인 딕셔너리이거나 `/Properties` 리소스에서 찾은 속성(찾지 못하면 None).
    pub properties: Option<Dictionary>,
    /// `/OC` 태그의 속성 이름이 가리키는 OCG·OCMD 객체.
    pub optional_content: Option<ObjectId>,
    pub mcid: Option<i64>,
    pub has_actual_text: bool,
}

/// 해석 중인 콘텐츠가 어디서 왔는지.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    Page(ObjectId),
    Form(ObjectId),
}

#[derive(Debug, Clone, PartialEq)]
pub enum XObjectKind {
    Image,
    Form,
    /// PostScript XObject나 `/Subtype`이 없는 손상 객체 — 그리지 않는다.
    Other,
}

/// 표시 연산자가 그리는 글자 뭉치의 크기 — 폰트 폭·높이로 계산한다.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShowExtent {
    /// 사용자 공간에서 글자들을 감싸는 사각형 `[left, bottom, right, top]`.
    pub bbox: [f64; 4],
    /// 텍스트 공간 가로 이동량(`Th` 적용 후) — 다음 글자 위치.
    pub advance: f64,
    /// 폰트 폭을 어림으로 잡았는지(표준 14 폰트 등) — 위치 보존 계산에는 쓸 수 없다.
    pub approximate: bool,
}

/// 연산자 하나를 볼 때의 해석기 상태.
pub struct Context<'a> {
    pub source: Source,
    /// 이 콘텐츠에서 쓰는 리소스(Form에 `/Resources`가 없으면 호출한 쪽 것).
    pub resources: &'a Dictionary,
    pub state: &'a GraphicsState,
    pub in_text_object: bool,
    pub text_matrix: &'a Matrix,
    pub text_line_matrix: &'a Matrix,
    pub marked: &'a [MarkedContent],
    /// Form 중첩 깊이(페이지 콘텐츠가 0).
    pub depth: usize,
    /// 표시 연산자(`Tj`, `TJ`, `'`, `"`)일 때 그 글자들의 크기. 폰트를 읽지 못하면 None.
    pub show: Option<ShowExtent>,
}

pub trait Visitor {
    /// 연산자를 적용하기 전에 불린다.
    fn operation(&mut self, _context: &Context, _index: usize, _op: &Operation) {}

    /// `Do`가 XObject를 가리킬 때(연산자 콜백 뒤). Form이면 이 뒤에 그 내용을 해석한다.
    fn xobject(&mut self, _context: &Context, _name: &[u8], _id: Option<ObjectId>, _kind: &XObjectKind) {}

    /// Form XObject에 들어가기 전. `false`를 돌려주면 그 Form을 해석하지 않는다.
    fn enter_form(&mut self, _id: ObjectId, _state: &GraphicsState) -> bool {
        true
    }

    fn exit_form(&mut self, _id: ObjectId) {}

    /// 콘텐츠를 읽지 못함(토크나이저 오류, 압축 해제 실패, 순환 참조, 깊이 초과).
    fn problem(&mut self, _source: Source, _problem: &Problem) {}
}

#[derive(Debug, Clone, PartialEq)]
pub enum Problem {
    Lex(LexError),
    Stream(String),
    Cycle(ObjectId),
    TooDeep(ObjectId),
}

/// Form XObject 최대 중첩 깊이 — 손상 파일의 무한 재귀 방지.
pub const MAX_FORM_DEPTH: usize = 32;

/// 페이지 한 장을 해석한다. 페이지 콘텐츠 자체를 읽지 못하면 `Err`(그 페이지는 건드리지 말 것).
/// Form 안에서 생긴 문제는 방문자의 `problem`으로 알린다.
pub fn interpret_page(doc: &Document, page_id: ObjectId, visitor: &mut dyn Visitor) -> Result<(), Problem> {
    let data = super::page_content_bytes(doc, page_id).map_err(|e| Problem::Stream(e.to_string()))?;
    let operations = tokenize(&data).map_err(Problem::Lex)?;
    let resources = page_resources(doc, page_id);
    let mut interpreter = Interpreter { doc, visitor, form_stack: HashSet::new(), fonts: Default::default() };
    interpreter.run(Source::Page(page_id), &operations, &resources, GraphicsState::default(), 0);
    Ok(())
}

/// 페이지의 리소스(Pages 트리 상속 포함, 없으면 빈 딕셔너리).
pub fn page_resources(doc: &Document, page_id: ObjectId) -> Dictionary {
    doc.get_dictionary(page_id)
        .ok()
        .and_then(|page| crate::geometry::inherited(doc, page, b"Resources"))
        .and_then(|o| resolve(doc, o).as_dict().ok())
        .cloned()
        .unwrap_or_default()
}

/// 리소스 딕셔너리의 하위 분류(`/Font`, `/XObject` 등)에서 이름으로 값을 찾는다.
pub fn resource_entry<'a>(doc: &'a Document, resources: &'a Dictionary, category: &[u8], name: &[u8]) -> Option<&'a Object> {
    let sub = resolve(doc, resources.get(category).ok()?).as_dict().ok()?;
    sub.get(name).ok()
}

struct Interpreter<'a, 'v> {
    doc: &'a Document,
    visitor: &'v mut dyn Visitor,
    /// 지금 해석 중인 Form 체인(순환 감지).
    form_stack: HashSet<ObjectId>,
    /// 폰트 객체별 폭·높이(같은 폰트를 여러 번 읽지 않게).
    fonts: std::collections::HashMap<ObjectId, Option<crate::fonts::FontInfo>>,
}

/// 경로를 만드는 연산자가 쌓는 좌표(사용자 공간)의 사각형.
#[derive(Default)]
struct PathBox {
    bbox: Option<[f64; 4]>,
    current: (f64, f64),
}

impl PathBox {
    fn add(&mut self, point: (f64, f64)) {
        self.current = point;
        self.bbox = Some(match self.bbox {
            None => [point.0, point.1, point.0, point.1],
            Some(b) => [b[0].min(point.0), b[1].min(point.1), b[2].max(point.0), b[3].max(point.1)],
        });
    }

    fn take(&mut self) -> Option<[f64; 4]> {
        self.current = (0.0, 0.0);
        self.bbox.take()
    }
}

pub fn intersect_box(a: Option<[f64; 4]>, b: [f64; 4]) -> Option<[f64; 4]> {
    let Some(a) = a else { return Some(b) };
    Some([a[0].max(b[0]), a[1].max(b[1]), a[2].min(b[2]), a[3].min(b[3])])
}

/// `b`가 `a` 안에 완전히 들어가는지(빈 사각형이면 거짓).
pub fn contains_box(a: &[f64; 4], b: &[f64; 4]) -> bool {
    b[0] >= a[0] && b[1] >= a[1] && b[2] <= a[2] && b[3] <= a[3] && b[2] > b[0] && b[3] > b[1]
}

/// 두 사각형이 겹치는지.
pub fn boxes_overlap(a: &[f64; 4], b: &[f64; 4]) -> bool {
    a[0] < b[2] && b[0] < a[2] && a[1] < b[3] && b[1] < a[3]
}

impl Interpreter<'_, '_> {
    fn run(&mut self, source: Source, operations: &[Operation], resources: &Dictionary, initial: GraphicsState, depth: usize) {
        let mut state = initial;
        let mut stack: Vec<GraphicsState> = Vec::new();
        let mut in_text = false;
        let mut tm = IDENTITY;
        let mut tlm = IDENTITY;
        let mut marked: Vec<MarkedContent> = Vec::new();
        let mut path = PathBox::default();
        let mut pending_clip = false;

        for (index, op) in operations.iter().enumerate() {
            // `'`와 `"`는 글자를 그리기 전에 줄을 먼저 바꾼다 — 그 뒤 위치로 글자 상자를 계산한다.
            let show_matrix = match op.operator.as_slice() {
                b"'" | b"\"" => multiply(&[1.0, 0.0, 0.0, 1.0, 0.0, -state.text.leading], &tlm),
                _ => tm,
            };
            let show = self.show_extent(resources, &state, &show_matrix, op);
            {
                let context = Context {
                    source,
                    resources,
                    state: &state,
                    in_text_object: in_text,
                    text_matrix: &tm,
                    text_line_matrix: &tlm,
                    marked: &marked,
                    depth,
                    show,
                };
                self.visitor.operation(&context, index, op);
            }

            let f = |i: usize| op.operand_f64(i).unwrap_or(0.0);
            let point = |i: usize| transform_point(&state.ctm, f(i), f(i + 1));
            match op.operator.as_slice() {
                b"q" => stack.push(state.clone()),
                // 짝이 맞지 않는 Q(빈 스택)는 무시한다 — 실제 파일에 있다.
                b"Q" => {
                    if let Some(saved) = stack.pop() {
                        state = saved;
                    }
                }
                b"cm" if op.operands.len() == 6 => {
                    let m = [f(0), f(1), f(2), f(3), f(4), f(5)];
                    state.ctm = multiply(&m, &state.ctm);
                }
                b"gs" => {
                    if let Some(name) = op.operand_name(0) {
                        self.apply_ext_gstate(resources, name, &mut state);
                    }
                }
                b"Tc" => state.text.char_spacing = f(0),
                b"Tw" => state.text.word_spacing = f(0),
                b"Tz" => state.text.horizontal_scaling = f(0),
                b"TL" => state.text.leading = f(0),
                b"Ts" => state.text.rise = f(0),
                b"Tr" => state.text.render_mode = f(0) as i64,
                b"Tf" => {
                    state.text.font_name = op.operand_name(0).map(<[u8]>::to_vec);
                    state.text.font_ref = None;
                    state.text.font_size = f(1);
                }
                b"BT" => {
                    in_text = true;
                    tm = IDENTITY;
                    tlm = IDENTITY;
                }
                b"ET" => in_text = false,
                b"Td" => {
                    tlm = multiply(&[1.0, 0.0, 0.0, 1.0, f(0), f(1)], &tlm);
                    tm = tlm;
                }
                b"TD" => {
                    state.text.leading = -f(1);
                    tlm = multiply(&[1.0, 0.0, 0.0, 1.0, f(0), f(1)], &tlm);
                    tm = tlm;
                }
                b"Tm" if op.operands.len() == 6 => {
                    tlm = [f(0), f(1), f(2), f(3), f(4), f(5)];
                    tm = tlm;
                }
                b"T*" | b"'" => {
                    tlm = multiply(&[1.0, 0.0, 0.0, 1.0, 0.0, -state.text.leading], &tlm);
                    tm = tlm;
                }
                b"\"" => {
                    state.text.word_spacing = f(0);
                    state.text.char_spacing = f(1);
                    tlm = multiply(&[1.0, 0.0, 0.0, 1.0, 0.0, -state.text.leading], &tlm);
                    tm = tlm;
                }
                b"g" => set_color(&mut state.fill_color_space, &mut state.fill_color, b"DeviceGray", op),
                b"G" => set_color(&mut state.stroke_color_space, &mut state.stroke_color, b"DeviceGray", op),
                b"rg" => set_color(&mut state.fill_color_space, &mut state.fill_color, b"DeviceRGB", op),
                b"RG" => set_color(&mut state.stroke_color_space, &mut state.stroke_color, b"DeviceRGB", op),
                b"k" => set_color(&mut state.fill_color_space, &mut state.fill_color, b"DeviceCMYK", op),
                b"K" => set_color(&mut state.stroke_color_space, &mut state.stroke_color, b"DeviceCMYK", op),
                b"cs" => {
                    state.fill_color_space = op.operand_name(0).unwrap_or(b"DeviceGray").to_vec();
                    state.fill_color = initial_color(&state.fill_color_space);
                }
                b"CS" => {
                    state.stroke_color_space = op.operand_name(0).unwrap_or(b"DeviceGray").to_vec();
                    state.stroke_color = initial_color(&state.stroke_color_space);
                }
                b"sc" | b"scn" => state.fill_color = op.operands.iter().filter_map(Operand::as_f64).collect(),
                b"SC" | b"SCN" => state.stroke_color = op.operands.iter().filter_map(Operand::as_f64).collect(),
                b"BMC" => marked.push(MarkedContent {
                    tag: op.operand_name(0).unwrap_or_default().to_vec(),
                    properties: None,
                    optional_content: None,
                    mcid: None,
                    has_actual_text: false,
                }),
                b"BDC" => marked.push(self.marked_content(resources, op)),
                b"EMC" => {
                    marked.pop();
                }
                b"m" | b"l" => path.add(point(0)),
                b"c" => (0..3).for_each(|i| path.add(point(i * 2))),
                b"v" | b"y" => (0..2).for_each(|i| path.add(point(i * 2))),
                b"re" => {
                    let (x, y, w, h) = (f(0), f(1), f(2), f(3));
                    for (px, py) in [(x, y), (x + w, y), (x + w, y + h), (x, y + h)] {
                        path.add(transform_point(&state.ctm, px, py));
                    }
                }
                b"W" | b"W*" => pending_clip = true,
                // 경로를 그리거나 버리는 연산자 — 대기 중인 클리핑이 있으면 여기서 적용된다.
                b"n" | b"f" | b"F" | b"f*" | b"S" | b"s" | b"B" | b"B*" | b"b" | b"b*" => {
                    let bbox = path.take();
                    if std::mem::take(&mut pending_clip) {
                        if let Some(bbox) = bbox {
                            state.clip = intersect_box(state.clip, bbox);
                        }
                    }
                }
                b"Do" => {
                    if let Some(name) = op.operand_name(0) {
                        let context = Context {
                            source,
                            resources,
                            state: &state,
                            in_text_object: in_text,
                            text_matrix: &tm,
                            text_line_matrix: &tlm,
                            marked: &marked,
                            depth,
                            show: None,
                        };
                        self.do_xobject(&context, name);
                    }
                }
                _ => {}
            }

            // 글자를 그린 만큼 텍스트 행렬을 옮긴다(`'`·`"`는 위에서 줄을 바꾼 뒤).
            if let Some(extent) = show {
                tm = multiply(&[1.0, 0.0, 0.0, 1.0, extent.advance, 0.0], &tm);
            }
        }
    }

    /// 표시 연산자가 그리는 글자 뭉치의 크기(폰트를 읽지 못하면 None).
    fn show_extent(&mut self, resources: &Dictionary, state: &GraphicsState, tm: &Matrix, op: &Operation) -> Option<ShowExtent> {
        if !matches!(op.operator.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"") {
            return None;
        }
        let text = &state.text;
        let doc = self.doc;
        let font = match (&text.font_name, text.font_ref) {
            (_, Some(id)) => self
                .fonts
                .entry(id)
                .or_insert_with(|| doc.get_dictionary(id).ok().and_then(|d| crate::fonts::FontInfo::load(doc, d)))
                .clone()?,
            (Some(name), None) => match resource_entry(doc, resources, b"Font", name)? {
                Object::Reference(id) => {
                    let id = *id;
                    self.fonts
                        .entry(id)
                        .or_insert_with(|| doc.get_dictionary(id).ok().and_then(|d| crate::fonts::FontInfo::load(doc, d)))
                        .clone()?
                }
                other => crate::fonts::FontInfo::load(doc, other.as_dict().ok()?)?,
            },
            (None, None) => return None,
        };
        let advance = font.metrics.show_advance(
            &op.operator,
            &op.operands,
            text.font_size,
            text.char_spacing,
            text.word_spacing,
            text.horizontal_scaling,
        )?;
        // 텍스트 공간 사각형(가로는 이동량, 세로는 글자 높이) → 사용자 공간.
        let (y0, y1) = (font.descent * text.font_size + text.rise, font.ascent * text.font_size + text.rise);
        let matrix = multiply(tm, &state.ctm);
        let corners = [(0.0, y0), (advance, y0), (0.0, y1), (advance, y1)]
            .map(|(x, y)| transform_point(&matrix, x, y));
        let bbox = corners.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, (x, y)| {
            [b[0].min(*x), b[1].min(*y), b[2].max(*x), b[3].max(*y)]
        });
        Some(ShowExtent { bbox, advance, approximate: font.metrics.is_approximate() })
    }

    fn apply_ext_gstate(&self, resources: &Dictionary, name: &[u8], state: &mut GraphicsState) {
        let doc = self.doc;
        let Some(dict) = resource_entry(doc, resources, b"ExtGState", name).and_then(|o| resolve(doc, o).as_dict().ok()) else {
            return;
        };
        if let Some(v) = dict.get(b"ca").ok().and_then(|o| number(resolve(doc, o))) {
            state.fill_alpha = v;
        }
        if let Some(v) = dict.get(b"CA").ok().and_then(|o| number(resolve(doc, o))) {
            state.stroke_alpha = v;
        }
        if let Ok(mask) = dict.get(b"SMask") {
            state.soft_mask = !matches!(resolve(doc, mask), Object::Name(n) if n == b"None");
        }
        // /Font [폰트참조 크기]
        if let Some(font) = dict.get(b"Font").ok().and_then(|o| resolve(doc, o).as_array().ok()) {
            if let (Some(Object::Reference(id)), Some(size)) = (font.first(), font.get(1).and_then(|o| number(resolve(doc, o)))) {
                state.text.font_name = None;
                state.text.font_ref = Some(*id);
                state.text.font_size = size;
            }
        }
    }

    fn marked_content(&self, resources: &Dictionary, op: &Operation) -> MarkedContent {
        let doc = self.doc;
        let tag = op.operand_name(0).unwrap_or_default().to_vec();
        let (properties, named_ref) = match op.operands.get(1) {
            Some(Operand::Name(name)) => {
                let object = resource_entry(doc, resources, b"Properties", name);
                let reference = object.and_then(|o| o.as_reference().ok());
                (object.and_then(|o| resolve(doc, o).as_dict().ok()).cloned(), reference)
            }
            Some(Operand::Dict(entries)) => (Some(operand_dict_to_lopdf(entries)), None),
            _ => (None, None),
        };
        let optional_content = if tag == b"OC" { named_ref } else { None };
        let mcid = properties
            .as_ref()
            .and_then(|p| p.get(b"MCID").ok())
            .and_then(|o| o.as_i64().ok());
        let has_actual_text = properties.as_ref().is_some_and(|p| p.has(b"ActualText"));
        MarkedContent { tag, properties, optional_content, mcid, has_actual_text }
    }

    fn do_xobject(&mut self, context: &Context, name: &[u8]) {
        let doc = self.doc;
        let object = resource_entry(doc, context.resources, b"XObject", name);
        let id = object.and_then(|o| o.as_reference().ok());
        let stream = object.and_then(|o| resolve(doc, o).as_stream().ok());
        let kind = match stream.and_then(|s| s.dict.get(b"Subtype").ok()).and_then(|o| o.as_name().ok()) {
            Some(b"Image") => XObjectKind::Image,
            Some(b"Form") => XObjectKind::Form,
            _ => XObjectKind::Other,
        };
        self.visitor.xobject(context, name, id, &kind);

        let (XObjectKind::Form, Some(stream), Some(id)) = (&kind, stream, id) else {
            return;
        };
        let source = Source::Form(id);
        if self.form_stack.contains(&id) {
            self.visitor.problem(source, &Problem::Cycle(id));
            return;
        }
        if context.depth + 1 > MAX_FORM_DEPTH {
            self.visitor.problem(source, &Problem::TooDeep(id));
            return;
        }
        if !self.visitor.enter_form(id, context.state) {
            return;
        }

        let data = match super::stream_bytes(doc, id) {
            Ok(data) => data,
            Err(e) => {
                self.visitor.problem(source, &Problem::Stream(e.to_string()));
                self.visitor.exit_form(id);
                return;
            }
        };
        match tokenize(&data) {
            Ok(operations) => {
                // Form은 자기 /Resources를 먼저, 없으면 호출한 쪽 리소스를 쓴다(구식이지만 실재).
                let resources = stream
                    .dict
                    .get(b"Resources")
                    .ok()
                    .and_then(|o| resolve(doc, o).as_dict().ok())
                    .cloned()
                    .unwrap_or_else(|| context.resources.clone());
                let matrix = stream
                    .dict
                    .get(b"Matrix")
                    .ok()
                    .and_then(|o| resolve(doc, o).as_array().ok())
                    .filter(|a| a.len() == 6)
                    .and_then(|a| {
                        let v: Vec<f64> = a.iter().filter_map(|o| number(resolve(doc, o))).collect();
                        (v.len() == 6).then(|| [v[0], v[1], v[2], v[3], v[4], v[5]])
                    })
                    .unwrap_or(IDENTITY);
                // Do는 q … Q로 감싼 것과 같다 — 호출 쪽 상태를 복사해 넘기므로 자동으로 복원된다.
                let mut inner = context.state.clone();
                inner.ctm = multiply(&matrix, &inner.ctm);
                self.form_stack.insert(id);
                self.run(source, &operations, &resources, inner, context.depth + 1);
                self.form_stack.remove(&id);
            }
            Err(e) => self.visitor.problem(source, &Problem::Lex(e)),
        }
        self.visitor.exit_form(id);
    }
}

fn set_color(space: &mut Vec<u8>, color: &mut Vec<f64>, new_space: &[u8], op: &Operation) {
    *space = new_space.to_vec();
    *color = op.operands.iter().filter_map(Operand::as_f64).collect();
}

/// `cs`/`CS`로 색 공간을 바꿨을 때의 초기 색(규격 8.6.8). 알 수 없는 공간은 빈 값.
fn initial_color(space: &[u8]) -> Vec<f64> {
    match space {
        b"DeviceGray" | b"CalGray" => vec![0.0],
        b"DeviceRGB" | b"CalRGB" | b"Lab" => vec![0.0, 0.0, 0.0],
        b"DeviceCMYK" => vec![0.0, 0.0, 0.0, 1.0],
        _ => Vec::new(),
    }
}

fn operand_dict_to_lopdf(entries: &[(Vec<u8>, Operand)]) -> Dictionary {
    let mut dict = Dictionary::new();
    for (key, value) in entries {
        dict.set(key.clone(), operand_to_object(value));
    }
    dict
}

fn operand_to_object(operand: &Operand) -> Object {
    match operand {
        Operand::Null => Object::Null,
        Operand::Bool(b) => Object::Boolean(*b),
        Operand::Int(i) => Object::Integer(*i),
        Operand::Real(r) => Object::Real(*r as f32),
        Operand::Name(n) => Object::Name(n.clone()),
        Operand::Str(s) => Object::String(s.clone(), lopdf::StringFormat::Literal),
        Operand::Array(items) => Object::Array(items.iter().map(operand_to_object).collect()),
        Operand::Dict(entries) => Object::Dictionary(operand_dict_to_lopdf(entries)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lopdf::{dictionary, Stream};

    /// 합성 PDF: 페이지 하나에 주어진 콘텐츠와 XObject들. 테스트 공용.
    pub(crate) fn one_page_doc(content: &[u8], xobjects: Vec<(&str, Stream)>) -> (Document, ObjectId) {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let mut xobject_dict = Dictionary::new();
        for (name, stream) in xobjects {
            let id = doc.add_object(stream);
            xobject_dict.set(name, id);
        }
        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.to_vec()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Contents" => content_id,
            "Resources" => dictionary! {
                "XObject" => xobject_dict,
                "ExtGState" => dictionary! { "Clear" => dictionary! { "ca" => 0 } },
            },
        });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! { "Type" => "Pages", "Kids" => vec![page_id.into()], "Count" => 1 }),
        );
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog);
        (doc, page_id)
    }

    pub(crate) fn form(content: &[u8]) -> Stream {
        Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Form", "BBox" => vec![0.into(), 0.into(), 612.into(), 792.into()] }, content.to_vec())
    }

    /// 표시 연산자마다 (출처, 렌더 모드, 채우기 알파, 깊이)를 모은다.
    #[derive(Default)]
    struct ShowRecorder {
        shows: Vec<(Source, i64, f64, usize)>,
        problems: Vec<Problem>,
        images: Vec<Matrix>,
    }

    impl Visitor for ShowRecorder {
        fn operation(&mut self, c: &Context, _index: usize, op: &Operation) {
            if matches!(op.operator.as_slice(), b"Tj" | b"TJ" | b"'" | b"\"") {
                self.shows.push((c.source, c.state.text.render_mode, c.state.fill_alpha, c.depth));
            }
        }
        fn xobject(&mut self, c: &Context, _name: &[u8], _id: Option<ObjectId>, kind: &XObjectKind) {
            if *kind == XObjectKind::Image {
                self.images.push(c.state.ctm);
            }
        }
        fn problem(&mut self, _source: Source, problem: &Problem) {
            self.problems.push(problem.clone());
        }
    }

    #[test]
    fn render_mode_persists_across_et_and_q_restores() {
        let (doc, page) = one_page_doc(b"3 Tr BT (a) Tj ET BT (b) Tj ET q 0 Tr BT (c) Tj ET Q BT (d) Tj ET", vec![]);
        let mut r = ShowRecorder::default();
        interpret_page(&doc, page, &mut r).unwrap();
        let modes: Vec<i64> = r.shows.iter().map(|s| s.1).collect();
        assert_eq!(modes, vec![3, 3, 0, 3]);
    }

    #[test]
    fn unbalanced_q_is_ignored() {
        let (doc, page) = one_page_doc(b"Q Q 3 Tr q Q Q BT (x) Tj ET", vec![]);
        let mut r = ShowRecorder::default();
        interpret_page(&doc, page, &mut r).unwrap();
        assert_eq!(r.shows[0].1, 3);
    }

    #[test]
    fn form_inherits_caller_state_and_ext_gstate_alpha() {
        let (doc, page) = one_page_doc(
            b"3 Tr /X0 Do 0 Tr /Clear gs /X0 Do",
            vec![("X0", form(b"BT (inner) Tj ET"))],
        );
        let mut r = ShowRecorder::default();
        interpret_page(&doc, page, &mut r).unwrap();
        assert_eq!(r.shows.len(), 2);
        assert_eq!((r.shows[0].1, r.shows[0].3), (3, 1));
        assert!(matches!(r.shows[0].0, Source::Form(_)));
        assert_eq!((r.shows[1].1, r.shows[1].2), (0, 0.0));
    }

    #[test]
    fn self_referencing_form_is_reported_not_looped() {
        let mut doc_and_page = one_page_doc(b"/X0 Do", vec![]);
        let doc = &mut doc_and_page.0;
        // X0이 자기 자신을 다시 부르는 손상 파일.
        let form_id = doc.new_object_id();
        let mut stream = form(b"/X0 Do");
        stream.dict.set("Resources", dictionary! { "XObject" => dictionary! { "X0" => form_id } });
        doc.objects.insert(form_id, Object::Stream(stream));
        let page = doc_and_page.1;
        let resources = doc.get_dictionary_mut(page).unwrap().get_mut(b"Resources").unwrap().as_dict_mut().unwrap();
        resources.set("XObject", dictionary! { "X0" => form_id });
        let mut r = ShowRecorder::default();
        interpret_page(&doc_and_page.0, page, &mut r).unwrap();
        assert_eq!(r.problems, vec![Problem::Cycle(form_id)]);
    }

    #[test]
    fn image_placement_uses_ctm() {
        let image = Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Image" }, vec![]);
        let (doc, page) = one_page_doc(b"q 612 0 0 792 0 0 cm /Im0 Do Q", vec![("Im0", image)]);
        let mut r = ShowRecorder::default();
        interpret_page(&doc, page, &mut r).unwrap();
        assert_eq!(r.images, vec![[612.0, 0.0, 0.0, 792.0, 0.0, 0.0]]);
    }

    #[test]
    fn broken_page_content_is_an_error() {
        let (doc, page) = one_page_doc(b"BT (unterminated Tj ET", vec![]);
        let mut r = ShowRecorder::default();
        assert!(matches!(interpret_page(&doc, page, &mut r), Err(Problem::Lex(_))));
    }

    /// 표시 연산자의 글자 상자와 클리핑 영역을 받아 두는 방문자.
    #[derive(Default)]
    struct BoxRecorder {
        shows: Vec<([f64; 4], Option<[f64; 4]>)>,
    }

    impl Visitor for BoxRecorder {
        fn operation(&mut self, c: &Context, _index: usize, _op: &Operation) {
            if let Some(show) = c.show {
                self.shows.push((show.bbox, c.state.clip));
            }
        }
    }

    /// 폭 500(1/1000 em), ascent 0.8, descent -0.2인 단순 폰트를 가진 페이지.
    fn doc_with_font(content: &[u8]) -> (Document, ObjectId) {
        let (mut doc, page) = one_page_doc(content, vec![]);
        let descriptor = doc.add_object(dictionary! { "Ascent" => 800, "Descent" => -200 });
        let font = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "T",
            "FirstChar" => 65, "Widths" => vec![500.into(), 500.into()], "FontDescriptor" => descriptor,
        });
        doc.get_dictionary_mut(page)
            .unwrap()
            .get_mut(b"Resources")
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Font", dictionary! { "F1" => font });
        (doc, page)
    }

    #[test]
    fn show_extent_uses_font_metrics_and_advances() {
        // 10pt 글자 두 개(각 폭 5pt) — 상자는 x 20~30, y 8~12(기준선 10 + ascent/descent)
        let (doc, page) = doc_with_font(b"BT /F1 10 Tf 1 0 0 1 20 10 Tm (AB) Tj (A) Tj ET");
        let mut r = BoxRecorder::default();
        interpret_page(&doc, page, &mut r).unwrap();
        assert_eq!(r.shows.len(), 2);
        assert_eq!(r.shows[0].0, [20.0, 8.0, 30.0, 18.0]);
        // 두 번째 Tj는 첫 번째가 그린 만큼(10pt) 오른쪽에서 시작한다
        assert_eq!(r.shows[1].0, [30.0, 8.0, 35.0, 18.0]);
    }

    #[test]
    fn clip_is_tracked_and_restored() {
        let (doc, page) = doc_with_font(
            b"q 10 10 100 50 re W n BT /F1 10 Tf 1 0 0 1 20 20 Tm (A) Tj ET Q BT /F1 10 Tf 1 0 0 1 20 20 Tm (A) Tj ET",
        );
        let mut r = BoxRecorder::default();
        interpret_page(&doc, page, &mut r).unwrap();
        assert_eq!(r.shows[0].1, Some([10.0, 10.0, 110.0, 60.0]));
        assert_eq!(r.shows[1].1, None, "Q로 클리핑이 풀린다");
    }

    #[test]
    fn matrix_multiply_order() {
        // 이동 뒤 2배 확대: (1,1) → (11,11) → (22,22)
        let translate = [1.0, 0.0, 0.0, 1.0, 10.0, 10.0];
        let scale = [2.0, 0.0, 0.0, 2.0, 0.0, 0.0];
        assert_eq!(transform_point(&multiply(&translate, &scale), 1.0, 1.0), (22.0, 22.0));
    }
}
