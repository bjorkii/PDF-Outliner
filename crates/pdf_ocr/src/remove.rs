//! OCR 전체 삭제(설계 문서 4장) — 표준 모드: 형태 A·B·D·E를 연산자 단위로 제거한다.
//! 형태 O(옛 리비전에 남은 OCR)는 전체 재작성(`save`)으로 함께 사라진다.
//!
//! 순서: [`plan`]으로 페이지마다 해석하고 고칠 스트림 내용을 미리 만든 뒤(문서는 아직 그대로),
//! [`apply`]로 문서에 반영한다. 검증에서 실패한 페이지는 [`Applied::rollback`]으로 되돌린다.
//!
//! **연산자 제거 규칙(4.2)** — `BT..ET`를 통째로 지우지 않는다. 블록 안에서 설정한 `Tf`, `Tr`,
//! 색 등은 `ET` 뒤에도 남아 뒤의 보이는 텍스트가 기댈 수 있기 때문이다. 표시 연산자만 지우고:
//! - `Tj`/`TJ`: 지운다. 같은 `BT` 안에서 위치를 다시 잡지 않고 남는 표시 연산자가 이어지면, 지운
//!   만큼의 이동을 `[n] TJ`로 채운다(`Tm`으로 채우면 `Tlm`까지 바뀌어 뒤의 `Td`가 틀어진다).
//! - `'` → `T*`, `"` → `aw Tw ac Tc T*`(+ 필요하면 `[n] TJ`).
//! - 이동량을 계산할 수 없으면(`fonts` 모듈 문서) 그 페이지는 건드리지 않는다.
//!
//! **공유 Form XObject** — Form은 호출한 쪽의 상태를 물려받아(`Do` 전의 `3 Tr` 등) 사용처마다
//! 결과가 다를 수 있다. 문서의 모든 사용처에서 결과가 같고 해석하지 않은 참조(주석 외형 등)가 없을
//! 때만 원본을 제자리에서 고친다. 그 밖에는 페이지 리소스에서 직접 부른 Form이면 그 페이지 전용
//! 복제본을 만들고, 다른 Form 안에서 부른 공유 Form이면 그 페이지를 건너뛴다(1차 범위).
//!
//! 고친 결과 아무것도 그리지 않게 된 Form은 내용을 비우고 `/Resources`를 떼어, 거기서만 쓰던
//! OCR 폰트가 저장 때 정리되게 한다.

use crate::classify::{classify, covered_by_image, is_show_operator, overlaps_image, HiddenKind, PageFacts, Verdict};
use crate::content::interp::{interpret_page, resource_entry, Context, GraphicsState, Problem, Source, Visitor, XObjectKind};
use crate::optional_content::OptionalContent;
use crate::content::lexer::{tokenize, Operation};
use crate::content::{page_content_bytes, stream_bytes};
use crate::fonts::{tj_number_for_advance, FontMetrics};
use crate::resources::{ensure_private_page_resources, page_subdict_mut, ReferenceCounts};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// 형태별 개수(`classify::HiddenKind`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KindCounts(BTreeMap<HiddenKind, usize>);

impl KindCounts {
    pub fn removed(&self) -> usize {
        self.0.values().sum()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, kind: HiddenKind) -> usize {
        self.0.get(&kind).copied().unwrap_or(0)
    }

    pub fn iter(&self) -> impl Iterator<Item = (HiddenKind, usize)> + '_ {
        self.0.iter().map(|(k, n)| (*k, *n))
    }

    pub fn add(&mut self, other: &KindCounts) {
        for (kind, n) in &other.0 {
            *self.0.entry(*kind).or_default() += n;
        }
    }

    fn count(&mut self, kind: HiddenKind) {
        *self.0.entry(kind).or_default() += 1;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PageStatus {
    /// 지울 것이 없음.
    Unchanged,
    Planned,
    /// 건드리지 않음(이유).
    Skipped(String),
}

#[derive(Debug, Clone)]
pub struct PagePlan {
    /// 1부터 센 페이지 번호.
    pub number: usize,
    pub page_id: ObjectId,
    /// 지운(지울) 형태별 개수.
    pub counts: KindCounts,
    /// 이 모드에서는 지우지 않고 보고만 하는 형태별 개수.
    pub reported: KindCounts,
    pub status: PageStatus,
    /// 처리는 했지만 알려야 할 것(해석하지 못한 Form 등).
    pub notes: Vec<String>,
    /// 지운 뒤 내용이 비게 된 태그 수(태그 PDF일 때).
    pub empty_tags: usize,
    content: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
struct FormVisit {
    form: ObjectId,
    page: usize,
    /// 1이면 페이지 콘텐츠에서 직접 부름.
    depth: usize,
    /// `Do`에 쓴 리소스 이름.
    name: Vec<u8>,
    /// 고친 내용(None이면 그대로).
    result: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq)]
enum FormDecision {
    /// 고친 내용과 이 Form을 쓰는 페이지들(0부터).
    InPlace(Vec<u8>, BTreeSet<usize>),
    /// 페이지 번호(0부터) → (리소스 이름, 내용).
    PerPage(BTreeMap<usize, (Vec<u8>, Vec<u8>)>),
}

pub struct RemovalPlan {
    pub pages: Vec<PagePlan>,
    forms: BTreeMap<ObjectId, FormDecision>,
}

impl RemovalPlan {
    pub fn totals(&self) -> KindCounts {
        let mut total = KindCounts::default();
        for page in &self.pages {
            if page.status == PageStatus::Planned {
                total.add(&page.counts);
            }
        }
        total
    }

    /// 지운 뒤 내용이 비게 되는 태그 수(계획에 든 페이지만).
    pub fn empty_tags(&self) -> usize {
        self.pages.iter().filter(|p| p.status == PageStatus::Planned).map(|p| p.empty_tags).sum()
    }

    /// 이 모드에서는 지우지 않고 보고만 하는 형태의 합(건너뛴 페이지 것도 포함).
    pub fn reported_totals(&self) -> KindCounts {
        let mut total = KindCounts::default();
        for page in &self.pages {
            total.add(&page.reported);
        }
        total
    }

    pub fn has_changes(&self) -> bool {
        self.pages.iter().any(|p| p.status == PageStatus::Planned)
    }
}

// ------------------------------------------------------------------ 해석·분류

/// 어떤 형태까지 지울지(설계 문서 4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// 자동 판정이 확실한 형태만(A·B·D·E).
    #[default]
    Standard,
    /// 휴리스틱 형태(F~K)까지. 렌더 비교로 화면이 그대로임을 확인해야 반영된다.
    Aggressive,
}

impl Mode {
    fn removes(&self, kind: HiddenKind) -> bool {
        match self {
            Mode::Standard => kind.is_standard(),
            Mode::Aggressive => true,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Removal {
    /// 표시 연산자 — 지운 만큼의 이동을 `[n] TJ`로 채워야 할 수 있다(이동량, 글자 크기, 가로 배율).
    Show { advance: Option<f64>, font_size: f64, horizontal_scaling: f64 },
    /// 그냥 지우는 연산자(빈 껍데기가 된 `BDC`·`EMC`).
    Delete,
}

#[derive(Default)]
struct StreamVisit {
    removals: BTreeMap<usize, Removal>,
    counts: KindCounts,
    /// 이 모드에서는 지우지 않고 세기만 한 형태.
    reported: KindCounts,
    /// 지우고 나서 내용이 비게 된 태그(마크드 콘텐츠) 수.
    empty_tags: usize,
    problem: Option<String>,
}

struct Frame {
    /// 페이지 콘텐츠면 None.
    form: Option<(ObjectId, usize, Vec<u8>)>,
    visit: StreamVisit,
    /// 열려 있는 `/OC` 블록(바깥부터).
    oc_blocks: Vec<MarkedBlock>,
}

/// 마크드 콘텐츠 블록 — 레이어(`/OC`)는 안이 다 비면 껍데기도 지우고, 태그(`/MCID`)는 비게 되는
/// 것을 세어 보고한다(설계 문서 4.3 — 껍데기를 남기므로 구조 트리가 끊기지는 않는다).
struct MarkedBlock {
    /// `BDC`의 연산자 위치.
    index: usize,
    /// 기본으로 꺼진 레이어인지.
    hidden_layer: bool,
    /// 태그된 콘텐츠(`/MCID`가 있음).
    tagged: bool,
    /// 블록 안에 남는 그리기가 있는지.
    painted: bool,
    /// 블록 안에서 무언가를 지웠는지.
    removed: bool,
}

/// 이미지에 덮이는지(H) 나중에 판정할 표시 연산자.
struct PendingShow {
    frame: usize,
    index: usize,
    bbox: [f64; 4],
    order: usize,
    removal: Removal,
}

struct Collector<'a> {
    doc: &'a Document,
    mode: Mode,
    facts: PageFacts,
    optional_content: &'a OptionalContent,
    /// 지금 들어와 있는, 기본으로 꺼진 레이어 Form의 수.
    hidden_layer_forms: usize,
    frames: Vec<Frame>,
    /// 지금 해석 중인 프레임(`frames` 인덱스) — 마지막이 현재.
    stack: Vec<usize>,
    pending_name: Vec<u8>,
    fonts: HashMap<ObjectId, Option<FontMetrics>>,
    /// 페이지 전체에서 연산자가 실행된 순서.
    order: usize,
    /// 그려진 이미지: (순서, 사용자 공간 사각형, 불투명 여부). `UnderImage`(H) 판정용이라
    /// **여태 그려진 것만** 담긴다(순서를 봐야 하기 때문).
    images: Vec<(usize, [f64; 4], bool)>,
    /// 페이지의 **모든** 이미지 상자 — 사전 패스에서 미리 모은다(→ `ImageBoxes`).
    /// OCR 텍스트 판정에 쓴다(그리는 순서와 무관하게 "그 자리에 스캔이 있는가").
    scans: Vec<(usize, [f64; 4], bool)>,
    pending: Vec<PendingShow>,
}

/// **사전 패스** — 페이지의 이미지 상자만 모은다.
///
/// OCR 텍스트 판정("이미지 영역 안이거나 걸친 `3 Tr`")에는 **페이지의 모든 이미지**가 필요하다.
/// 실측(KKZ000160_01.pdf 등)에서 OCR 텍스트 Form이 스캔 이미지보다 **먼저** 그려지는 것을
/// 확인했다 — 스캔이 그 위를 덮는 구성이다. 그래서 "여태 그려진 이미지"만 보면 OCR을 통째로
/// 놓친다(9302건 → 0건이 되는 것을 실측으로 확인).
///
/// 판정을 페이지 끝으로 미루는 방법도 있지만, 그러면 지우기·`mark_painted`·빈 블록 정리가 얽힌
/// 섬세한 흐름을 고쳐야 한다. 페이지를 한 번 더 읽는 편이 훨씬 안전하다 — 해석기는 렌더링 없이
/// lopdf만 쓰므로 비용이 작고, 검증 단계의 렌더 비교가 어차피 시간을 지배한다.
#[derive(Default)]
struct ImageBoxes {
    /// (순서, 사용자 공간 사각형, 불투명 여부) — `Collector::images`와 같은 모양.
    images: Vec<(usize, [f64; 4], bool)>,
    order: usize,
}

impl Visitor for ImageBoxes {
    fn operation(&mut self, _context: &Context, _index: usize, _op: &Operation) {
        self.order += 1;
    }

    fn xobject(&mut self, context: &Context, _name: &[u8], _id: Option<ObjectId>, kind: &XObjectKind) {
        if matches!(kind, XObjectKind::Image { .. }) {
            self.images.push((self.order, Collector::image_box(context.state), true));
        }
    }
}

impl Collector<'_> {
    fn frame(&mut self) -> &mut StreamVisit {
        let index = *self.stack.last().expect("프레임 스택은 비지 않는다");
        &mut self.frames[index].visit
    }

    /// 표시 연산자의 이동량(폰트 폭). 지운 자리에 `[n] TJ`를 넣어야 할 때 쓴다.
    fn advance(&mut self, context: &Context, op: &Operation) -> Option<f64> {
        if let Some(show) = context.show {
            return (!show.approximate).then_some(show.advance);
        }
        // 해석기가 폰트를 못 읽은 경우(ExtGState 폰트 등)를 위한 보조 경로.
        let text = &context.state.text;
        let name = text.font_name.as_ref()?;
        let doc = self.doc;
        let metrics = match resource_entry(doc, context.resources, b"Font", name)? {
            Object::Reference(id) => {
                let id = *id;
                self.fonts
                    .entry(id)
                    .or_insert_with(|| doc.get_dictionary(id).ok().and_then(|d| FontMetrics::load(doc, d)))
                    .clone()?
            }
            other => FontMetrics::load(doc, other.as_dict().ok()?)?,
        };
        metrics.show_advance(
            &op.operator,
            &op.operands,
            text.font_size,
            text.char_spacing,
            text.word_spacing,
            text.horizontal_scaling,
        )
    }

    /// 마크드 콘텐츠 스택에 기본으로 꺼진 레이어가 있는지.
    fn in_hidden_layer(&self, context: &Context) -> bool {
        self.hidden_layer_forms > 0
            || context.marked.iter().any(|m| {
                m.optional_content.is_some_and(|id| self.optional_content.is_hidden(self.doc, id) == Some(true))
            })
    }

    /// XObject 딕셔너리의 `/OC`가 기본으로 꺼진 레이어를 가리키는지.
    fn xobject_hidden(&self, id: ObjectId) -> bool {
        self.doc
            .get_object(id)
            .ok()
            .and_then(|o| o.as_stream().ok())
            .and_then(|s| s.dict.get(b"OC").ok())
            .and_then(|o| o.as_reference().ok())
            .is_some_and(|oc| self.optional_content.is_hidden(self.doc, oc) == Some(true))
    }

    /// 이미지가 그 아래를 완전히 가리는지 — 마스크·투명도가 있으면 아니다.
    fn image_is_opaque(&self, dict: &Dictionary, state: &GraphicsState) -> bool {
        let has = |key: &[u8]| dict.get(key).is_ok_and(|o| !matches!(o, Object::Null));
        state.fill_alpha > 0.99 && !state.soft_mask && !has(b"SMask") && !has(b"Mask") && !has(b"IM") && !has(b"ImageMask")
    }

    /// 단위 정사각형을 CTM으로 옮긴 사각형(이미지가 놓인 자리).
    fn image_box(state: &GraphicsState) -> [f64; 4] {
        let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)]
            .map(|(x, y)| crate::content::interp::transform_point(&state.ctm, x, y));
        corners.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, (x, y)| {
            [b[0].min(*x), b[1].min(*y), b[2].max(*x), b[3].max(*y)]
        })
    }

    /// 이 표시 연산자가 스캔 이미지 위에 있는지 — OCR 텍스트 판정(→ `classify::overlaps_image`).
    ///
    /// 글자 상자를 구하지 못한 경우(폰트 정보를 읽을 수 없는 등)에는 **페이지에 이미지가 있는지**로
    /// 대신 본다. 모른다는 이유로 OCR 텍스트를 놓치는 쪽이 더 나쁘기 때문이다 — 글자 상자를 못
    /// 구하는 파일은 그대로 "처리할 수 없는 페이지"로 보고되어 사용자가 알게 된다.
    fn on_scan(&self, context: &Context) -> bool {
        match context.show {
            Some(show) => overlaps_image(&show.bbox, &self.scans),
            None => !self.scans.is_empty(),
        }
    }

    fn record(&mut self, kind: HiddenKind, index: usize, removal: Removal) {
        let mode = self.mode;
        if !mode.removes(kind) {
            let frame = *self.stack.last().expect("프레임");
            self.mark_painted(frame); // 남는 글자라 블록이 비지 않는다
            self.frame().reported.count(kind);
            return;
        }
        let frame = *self.stack.last().expect("프레임");
        for block in &mut self.frames[frame].oc_blocks {
            block.removed = true;
        }
        let visit = self.frame();
        visit.counts.count(kind);
        visit.removals.insert(index, removal);
    }

    /// 지금 열려 있는 모든 `/OC` 블록에 "그리는 것이 남았다"고 표시한다.
    fn mark_painted(&mut self, frame: usize) {
        for block in &mut self.frames[frame].oc_blocks {
            block.painted = true;
        }
    }
}

impl Visitor for Collector<'_> {
    fn operation(&mut self, context: &Context, index: usize, op: &Operation) {
        self.order += 1;
        if op.is(b"BI") {
            if let Some(image) = &op.inline_image {
                let opaque = context.state.fill_alpha > 0.99
                    && !context.state.soft_mask
                    && !image.dict.iter().any(|(k, _)| k == b"IM" || k == b"ImageMask" || k == b"SMask");
                self.images.push((self.order, Self::image_box(context.state), opaque));
            }
            return;
        }
        // 마크드 콘텐츠 블록 열고 닫기 — 꺼진 레이어는 안이 다 비면 껍데기까지 지우고,
        // 태그는 비게 된 개수를 센다.
        let frame_index = *self.stack.last().expect("프레임");
        if op.is(b"BDC") {
            // 방문자는 연산자를 적용하기 전에 불리므로 이 BDC는 아직 `context.marked`에 없다.
            let hidden_layer = op.operand_name(0) == Some(b"OC")
                && op
                    .operand_name(1)
                    .and_then(|name| resource_entry(self.doc, context.resources, b"Properties", name))
                    .and_then(|o| o.as_reference().ok())
                    .is_some_and(|id| self.optional_content.is_hidden(self.doc, id) == Some(true));
            let tagged = op.operands.get(1).and_then(|o| o.dict_get(b"MCID")).is_some()
                || op
                    .operand_name(1)
                    .and_then(|name| resource_entry(self.doc, context.resources, b"Properties", name))
                    .and_then(|o| crate::geometry::resolve(self.doc, o).as_dict().ok())
                    .is_some_and(|d| d.has(b"MCID"));
            self.frames[frame_index].oc_blocks.push(MarkedBlock { index, hidden_layer, tagged, painted: false, removed: false });
        } else if op.is(b"EMC") {
            if let Some(block) = self.frames[frame_index].oc_blocks.pop() {
                if !block.painted {
                    let visit = &mut self.frames[frame_index].visit;
                    if block.hidden_layer && self.mode.removes(HiddenKind::HiddenLayer) {
                        visit.removals.insert(block.index, Removal::Delete);
                        visit.removals.insert(index, Removal::Delete);
                    } else if block.tagged && block.removed {
                        visit.empty_tags += 1;
                    }
                }
            }
        } else if matches!(
            op.operator.as_slice(),
            b"Do" | b"sh" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*"
        ) {
            self.mark_painted(frame_index);
        }

        self.facts.in_hidden_layer = self.in_hidden_layer(context);
        let verdict = classify(context, op, &self.facts);
        if verdict == Verdict::Keep {
            if is_show_operator(op) {
                self.mark_painted(frame_index);
            }
            return;
        }
        let removal = Removal::Show {
            advance: self.advance(context, op),
            font_size: context.state.text.font_size,
            horizontal_scaling: context.state.text.horizontal_scaling,
        };
        match verdict {
            Verdict::Keep => {}
            // `3 Tr`은 그 자리에 스캔 이미지가 있을 때만 OCR 텍스트로 본다(→ `overlaps_image`).
            // 이미지와 무관한 `3 Tr`은 OCR이 만든 것이 아니므로 남긴다.
            Verdict::Hidden(HiddenKind::InvisibleMode) if !self.on_scan(context) => {
                self.mark_painted(frame_index); // 남는 글자라 블록이 비지 않는다
            }
            Verdict::Hidden(kind) => self.record(kind, index, removal),
            Verdict::MaybeUnderImage => {
                // 이미지가 나중에 덮는지는 페이지를 다 읽은 뒤에 판정한다. 그때까지는 남는
                // 글자로 보고 블록이 비지 않았다고 둔다(덮인 것으로 판정돼도 껍데기는 남긴다).
                self.mark_painted(frame_index);
                if let Some(show) = context.show {
                    self.pending.push(PendingShow { frame: frame_index, index, bbox: show.bbox, order: self.order, removal });
                }
            }
        }
    }

    fn xobject(&mut self, context: &Context, name: &[u8], id: Option<ObjectId>, kind: &XObjectKind) {
        self.pending_name = name.to_vec();
        if *kind != XObjectKind::Image {
            return;
        }
        let dict = id
            .and_then(|id| self.doc.get_object(id).ok())
            .and_then(|o| o.as_stream().ok())
            .map(|s| s.dict.clone())
            .unwrap_or_default();
        let opaque = self.image_is_opaque(&dict, context.state);
        self.images.push((self.order, Self::image_box(context.state), opaque));
    }

    fn enter_form(&mut self, id: ObjectId, _state: &GraphicsState) -> bool {
        let depth = self.stack.len();
        let name = std::mem::take(&mut self.pending_name);
        if self.xobject_hidden(id) {
            self.hidden_layer_forms += 1;
        }
        self.frames.push(Frame { form: Some((id, depth, name)), visit: StreamVisit::default(), oc_blocks: Vec::new() });
        self.stack.push(self.frames.len() - 1);
        true
    }

    fn exit_form(&mut self, id: ObjectId) {
        self.stack.pop();
        if self.xobject_hidden(id) {
            self.hidden_layer_forms -= 1;
        }
    }

    fn problem(&mut self, source: Source, problem: &Problem) {
        let message = match problem {
            Problem::Lex(e) => format!("콘텐츠 해석 실패({e})"),
            Problem::Stream(e) => e.clone(),
            Problem::Cycle(_) => "Form이 자기 자신을 다시 부름(손상)".to_string(),
            Problem::TooDeep(_) => "Form 중첩이 너무 깊음".to_string(),
        };
        // 해석하지 못한 Form은 그대로 둔다(그 안의 텍스트도 남는다).
        if let Source::Form(id) = source {
            if let Some(&frame) = self.stack.last() {
                if self.frames[frame].form.as_ref().is_some_and(|(fid, _, _)| *fid == id) {
                    let visit = &mut self.frames[frame].visit;
                    visit.problem = Some(message);
                    visit.removals.clear();
                    visit.counts = KindCounts::default();
                    return;
                }
            }
        }
        self.frame().problem.get_or_insert(message);
    }
}

// ------------------------------------------------------------------ 스트림 편집

/// `removals`의 연산자를 지운(또는 바꾼) 새 스트림 바이트.
fn edit_stream(bytes: &[u8], operations: &[Operation], removals: &BTreeMap<usize, Removal>) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    for (&index, removal) in removals {
        let op = operations.get(index).ok_or("연산자 위치가 맞지 않음")?;
        out.extend_from_slice(&bytes[cursor..op.span.start]);
        cursor = op.span.end;

        let Removal::Show { advance, font_size, horizontal_scaling } = *removal else {
            continue; // 그냥 지운다(빈 /OC 껍데기)
        };
        let mut replacement = match op.operator.as_slice() {
            b"'" => "T*".to_string(),
            b"\"" => {
                let aw = op.operand_f64(0).ok_or("\" 연산자 피연산자 오류")?;
                let ac = op.operand_f64(1).ok_or("\" 연산자 피연산자 오류")?;
                format!("{} Tw {} Tc T*", fmt_number(aw), fmt_number(ac))
            }
            _ => String::new(),
        };
        if needs_advance(operations, removals, index) {
            // 폭을 모르면 지운 자리만큼 뒤 글자가 당겨져 줄이 틀어진다 — 그 쪽은 그대로 둔다.
            let advance = advance.ok_or("글자 폭을 계산할 수 없는 폰트가 있어 삭제 후 레이아웃이 깨짐")?;
            let n = tj_number_for_advance(advance, font_size, horizontal_scaling)
                .ok_or("크기 0 폰트의 이동량을 보존할 수 없음")?;
            if !replacement.is_empty() {
                replacement.push(' ');
            }
            replacement.push_str(&format!("[{}] TJ", fmt_number(n)));
        }
        if !replacement.is_empty() {
            out.push(b' ');
            out.extend_from_slice(replacement.as_bytes());
            out.push(b' ');
        }
    }
    out.extend_from_slice(&bytes[cursor..]);
    Ok(out)
}

/// `index`의 표시 연산자를 지운 뒤, 위치를 다시 잡기 전에 남는 표시 연산자가 이어지는지.
fn needs_advance(operations: &[Operation], removals: &BTreeMap<usize, Removal>, index: usize) -> bool {
    for (j, op) in operations.iter().enumerate().skip(index + 1) {
        let removed = removals.contains_key(&j);
        match op.operator.as_slice() {
            b"'" | b"\"" | b"Td" | b"TD" | b"Tm" | b"T*" | b"ET" | b"BT" => return false,
            b"Tj" | b"TJ" if !removed => return true,
            _ => {}
        }
    }
    false
}

fn fmt_number(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" || s.is_empty() {
        "0".to_string()
    } else {
        s.to_string()
    }
}

/// 그리는 연산자가 하나도 없으면(상태 설정·빈 텍스트 블록만 남음) 참.
fn paints_nothing(bytes: &[u8]) -> bool {
    let Ok(operations) = tokenize(bytes) else { return false };
    !operations.iter().any(|op| {
        is_show_operator(op)
            || matches!(
                op.operator.as_slice(),
                b"Do" | b"BI" | b"sh" | b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"d0" | b"d1"
            )
    })
}

// ------------------------------------------------------------------ 계획

pub fn plan(doc: &Document) -> RemovalPlan {
    plan_for(doc, None, Mode::Standard)
}

/// `only`에 든 페이지(0부터)만 지우는 계획. 다른 페이지도 해석은 한다 — 공유 Form을 제자리에서
/// 고쳐도 되는지 판단하려면 모든 사용처를 알아야 하기 때문이다(그 페이지들의 사용처는 "그대로"로 친다).
pub fn plan_for(doc: &Document, only: Option<&BTreeSet<usize>>, mode: Mode) -> RemovalPlan {
    let counts = ReferenceCounts::count(doc);
    let optional_content = OptionalContent::load(doc);
    let page_ids: Vec<ObjectId> = doc.get_pages().into_values().collect();
    let mut pages = Vec::with_capacity(page_ids.len());
    let mut visits: Vec<FormVisit> = Vec::new();

    for (page_index, &page_id) in page_ids.iter().enumerate() {
        let mut page = PagePlan {
            number: page_index + 1,
            page_id,
            counts: KindCounts::default(),
            reported: KindCounts::default(),
            status: PageStatus::Unchanged,
            notes: Vec::new(),
            empty_tags: 0,
            content: None,
        };
        let crop = crate::geometry::PageFrame::from_page(doc, page_id)
            .map(|f| [f.crop.llx, f.crop.lly, f.crop.urx, f.crop.ury])
            .unwrap_or([f64::MIN / 4.0, f64::MIN / 4.0, f64::MAX / 4.0, f64::MAX / 4.0]);
        // 이미지 상자를 먼저 모은다(→ `ImageBoxes`). 실패하면 빈 목록으로 두고 넘어간다 —
        // 본 패스가 같은 오류를 만나 "처리할 수 없는 페이지"로 보고한다.
        let mut prepass = ImageBoxes::default();
        let _ = interpret_page(doc, page_id, &mut prepass);
        let scans = prepass.images;
        let mut collector = Collector {
            doc,
            mode,
            facts: PageFacts { crop, in_hidden_layer: false },
            optional_content: &optional_content,
            hidden_layer_forms: 0,
            frames: vec![Frame { form: None, visit: StreamVisit::default(), oc_blocks: Vec::new() }],
            stack: vec![0],
            pending_name: Vec::new(),
            fonts: HashMap::new(),
            order: 0,
            images: Vec::new(),
            scans,
            pending: Vec::new(),
        };
        if let Err(problem) = interpret_page(doc, page_id, &mut collector) {
            page.status = PageStatus::Skipped(match problem {
                Problem::Lex(e) => format!("페이지 콘텐츠 해석 실패({e})"),
                Problem::Stream(e) => format!("페이지 콘텐츠를 읽지 못함({e})"),
                other => format!("{other:?}"),
            });
            pages.push(page);
            continue;
        }
        // 이미지에 덮이는 텍스트(H)는 페이지를 다 읽은 뒤에 판정한다.
        let pending = std::mem::take(&mut collector.pending);
        for show in pending {
            if covered_by_image(&show.bbox, show.order, &collector.images) {
                let visit = &mut collector.frames[show.frame].visit;
                if mode.removes(HiddenKind::UnderImage) {
                    visit.counts.count(HiddenKind::UnderImage);
                    visit.removals.insert(show.index, show.removal);
                } else {
                    visit.reported.count(HiddenKind::UnderImage);
                }
            }
        }
        let mut frames = collector.frames;
        let page_frame = frames.remove(0);
        let page_visit = page_frame.visit;
        let mut skip: Option<String> = None;
        page.counts.add(&page_visit.counts);
        page.reported.add(&page_visit.reported);
        page.empty_tags += page_visit.empty_tags;
        if let Some(problem) = &page_visit.problem {
            page.notes.push(problem.clone());
        }
        if !page_visit.removals.is_empty() {
            match page_content_bytes(doc, page_id)
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    let operations = tokenize(&bytes).map_err(|e| e.to_string())?;
                    edit_stream(&bytes, &operations, &page_visit.removals)
                }) {
                Ok(content) => page.content = Some(content),
                Err(e) => skip = Some(e),
            }
        }
        let mut page_visits = Vec::new();
        for frame in frames {
            let (form, depth, name) = frame.form.expect("Form 프레임");
            let visit = frame.visit;
            page.counts.add(&visit.counts);
            page.reported.add(&visit.reported);
            page.empty_tags += visit.empty_tags;
            if let Some(problem) = &visit.problem {
                page.notes.push(format!("Form {form:?}: {problem} — 이 Form 안의 텍스트는 그대로 둠"));
            }
            let result = if visit.removals.is_empty() {
                None
            } else {
                match stream_bytes(doc, form).map_err(|e| e.to_string()).and_then(|bytes| {
                    let operations = tokenize(&bytes).map_err(|e| e.to_string())?;
                    edit_stream(&bytes, &operations, &visit.removals)
                }) {
                    Ok(content) => Some(content),
                    Err(e) => {
                        skip.get_or_insert(e);
                        None
                    }
                }
            };
            page_visits.push(FormVisit { form, page: page_index, depth, name, result });
        }
        if only.is_some_and(|set| !set.contains(&page_index)) {
            page.status = PageStatus::Unchanged;
            page.content = None;
            page.counts = KindCounts::default();
            page.reported = KindCounts::default();
            visits.extend(page_visits.into_iter().map(|v| FormVisit { result: None, ..v }));
            pages.push(page);
            continue;
        }
        match skip {
            Some(reason) => {
                page.status = PageStatus::Skipped(reason);
                page.content = None;
                // 건너뛴 페이지의 Form 사용처는 "원본 그대로"로 친다.
                visits.extend(page_visits.into_iter().map(|v| FormVisit { result: None, ..v }));
            }
            None => {
                if page.counts.removed() > 0 {
                    page.status = PageStatus::Planned;
                }
                visits.extend(page_visits);
            }
        }
        pages.push(page);
    }

    let forms = decide_forms(&counts, &mut pages, &mut visits);
    RemovalPlan { pages, forms }
}

/// Form마다 제자리 수정·페이지별 복제·건너뛰기를 정한다(모듈 문서 "공유 Form XObject").
fn decide_forms(
    counts: &ReferenceCounts,
    pages: &mut [PagePlan],
    visits: &mut [FormVisit],
) -> BTreeMap<ObjectId, FormDecision> {
    loop {
        let mut by_form: BTreeMap<ObjectId, Vec<usize>> = BTreeMap::new();
        for (i, v) in visits.iter().enumerate() {
            by_form.entry(v.form).or_default().push(i);
        }
        let mut decisions = BTreeMap::new();
        let mut newly_skipped: BTreeSet<usize> = BTreeSet::new();

        for (form, indices) in &by_form {
            if indices.iter().all(|&i| visits[i].result.is_none()) {
                continue;
            }
            let first = &visits[indices[0]].result;
            let all_same = indices.iter().all(|&i| &visits[i].result == first);
            let visited_pages: BTreeSet<usize> = indices.iter().map(|&i| visits[i].page).collect();
            // 해석하지 않은 참조가 있으면(주석 외형 등) 원본은 남겨야 한다.
            // 상속 Resources처럼 참조 하나를 여러 페이지가 쓰는 경우가 흔하므로, 참조 수가 방문한
            // 페이지 수보다 많을 때만 해석하지 않은 사용처가 있다고 본다.
            let unvisited_users = counts.get(*form) > visited_pages.len();
            if all_same && !unvisited_users {
                if let Some(content) = first {
                    decisions.insert(*form, FormDecision::InPlace(content.clone(), visited_pages));
                }
                continue;
            }
            let mut per_page: BTreeMap<usize, (Vec<u8>, Vec<u8>)> = BTreeMap::new();
            for &i in indices {
                let v = &visits[i];
                let Some(content) = &v.result else { continue };
                if v.depth != 1 {
                    newly_skipped.insert(v.page);
                    continue;
                }
                match per_page.get(&v.page) {
                    Some((name, existing)) if name != &v.name || existing != content => {
                        newly_skipped.insert(v.page);
                    }
                    _ => {
                        per_page.insert(v.page, (v.name.clone(), content.clone()));
                    }
                }
            }
            per_page.retain(|page, _| !newly_skipped.contains(page));
            if !per_page.is_empty() {
                decisions.insert(*form, FormDecision::PerPage(per_page));
            }
        }

        if newly_skipped.is_empty() {
            return decisions;
        }
        for page in newly_skipped {
            pages[page].status =
                PageStatus::Skipped("여러 곳에서 다르게 쓰이는 공유 Form이 다른 Form 안에 있음".to_string());
            pages[page].content = None;
            for v in visits.iter_mut().filter(|v| v.page == page) {
                v.result = None;
            }
        }
    }
}

// ------------------------------------------------------------------ 레이어 정리

/// 콘텐츠·주석에서 아직 쓰이는 레이어(OCG·OCMD와 그 구성원)를 모은다.
#[derive(Default)]
struct OcgCollector {
    used: std::collections::HashSet<ObjectId>,
}

impl Visitor for OcgCollector {
    fn operation(&mut self, context: &Context, _index: usize, _op: &Operation) {
        self.used.extend(context.marked.iter().filter_map(|m| m.optional_content));
    }

    fn xobject(&mut self, _context: &Context, _name: &[u8], id: Option<ObjectId>, _kind: &XObjectKind) {
        self.used.extend(id);
    }
}

/// 더 이상 쓰이지 않는 레이어를 `/OCProperties`에서 뺀다(설계 문서 4.4) — 남겨 두면 뷰어 레이어
/// 패널에 빈 레이어가 보인다. 지운 개수를 돌려준다.
pub fn prune_optional_content(doc: &mut Document) -> usize {
    let optional_content = OptionalContent::load(doc);
    if optional_content.is_empty() {
        return 0;
    }
    let mut collector = OcgCollector::default();
    let page_ids: Vec<ObjectId> = doc.get_pages().into_values().collect();
    for page_id in &page_ids {
        let _ = interpret_page(doc, *page_id, &mut collector);
        // 주석의 /OC와 XObject 딕셔너리의 /OC.
        if let Ok(page) = doc.get_dictionary(*page_id) {
            if let Some(annots) = page.get(b"Annots").ok().and_then(|o| crate::geometry::resolve(doc, o).as_array().ok()) {
                for annot in annots {
                    if let Some(oc) = crate::geometry::resolve(doc, annot)
                        .as_dict()
                        .ok()
                        .and_then(|a| a.get(b"OC").ok())
                        .and_then(|o| o.as_reference().ok())
                    {
                        collector.used.insert(oc);
                    }
                }
            }
        }
    }
    // XObject가 /OC로 가리키는 레이어, 그리고 OCMD가 품은 OCG까지 포함한다.
    let mut referenced = std::collections::HashSet::new();
    for id in collector.used {
        referenced.insert(id);
        if let Ok(dict) = doc.get_dictionary(id) {
            if let Ok(oc) = dict.get(b"OC").and_then(Object::as_reference) {
                referenced.insert(oc);
            }
            for member in ocmd_members(doc, dict) {
                referenced.insert(member);
            }
        }
        if let Some(members) = doc
            .get_object(id)
            .ok()
            .and_then(|o| o.as_stream().ok())
            .and_then(|s| s.dict.get(b"OC").ok())
            .and_then(|o| o.as_reference().ok())
        {
            referenced.insert(members);
            if let Ok(dict) = doc.get_dictionary(members) {
                referenced.extend(ocmd_members(doc, dict));
            }
        }
    }
    crate::optional_content::prune_unreferenced(doc, &referenced)
}

fn ocmd_members(doc: &Document, dict: &Dictionary) -> Vec<ObjectId> {
    match dict.get(b"OCGs").ok().map(|o| crate::geometry::resolve(doc, o)) {
        Some(Object::Array(items)) => items.iter().filter_map(|o| o.as_reference().ok()).collect(),
        _ => dict.get(b"OCGs").ok().and_then(|o| o.as_reference().ok()).into_iter().collect(),
    }
}

// ------------------------------------------------------------------ 적용·되돌리기

/// 적용 전 원본 — 되돌리기용. 삭제([`apply`]), 앱 레이어 떼어 내기, 레이어 삽입이 함께 기록한다
/// (페이지마다 가장 먼저 기록한 상태가 원본이다).
#[derive(Default)]
pub struct Applied {
    page_originals: HashMap<usize, Object>,
    /// 제자리에서 고친 Form의 원본과 그 Form을 쓰는 페이지들.
    form_originals: HashMap<ObjectId, (Object, BTreeSet<usize>)>,
}

pub fn apply(doc: &mut Document, plan: &RemovalPlan, applied: &mut Applied) -> anyhow::Result<()> {
    let mut counts = ReferenceCounts::count(doc);

    let planned: BTreeSet<usize> = plan
        .pages
        .iter()
        .enumerate()
        .filter(|(_, p)| p.status == PageStatus::Planned)
        .map(|(i, _)| i)
        .collect();
    for &i in &planned {
        applied.record_page(doc, i, plan.pages[i].page_id)?;
    }

    for (&form, decision) in &plan.forms {
        match decision {
            FormDecision::InPlace(content, users) => {
                let original = doc.get_object(form)?.clone();
                let users = users.clone();
                if applied.form_originals.contains_key(&form) {
                    // 이미 기록된 원본 유지(사용 페이지만 합친다)
                    if let Some((_, known)) = applied.form_originals.get_mut(&form) {
                        known.extend(users.iter().copied());
                    }
                    let stream = doc.get_object_mut(form)?.as_stream_mut()?;
                    set_form_content(stream, content.clone());
                    continue;
                }
                let stream = doc.get_object_mut(form)?.as_stream_mut()?;
                set_form_content(stream, content.clone());
                applied.form_originals.insert(form, (original, users));
            }
            FormDecision::PerPage(per_page) => {
                for (&page_index, (name, content)) in per_page {
                    if !planned.contains(&page_index) {
                        continue;
                    }
                    let mut stream = doc.get_object(form)?.as_stream()?.clone();
                    set_form_content(&mut stream, content.clone());
                    let new_id = doc.add_object(stream);
                    let page_id = plan.pages[page_index].page_id;
                    ensure_private_page_resources(doc, &mut counts, page_id)?;
                    page_subdict_mut(doc, &mut counts, page_id, b"XObject")?.set(name.clone(), new_id);
                }
            }
        }
    }

    for &i in &planned {
        let page = &plan.pages[i];
        if let Some(content) = &page.content {
            let mut stream = Stream::new(Dictionary::new(), content.clone());
            let _ = stream.compress();
            let id = doc.add_object(stream);
            doc.get_dictionary_mut(page.page_id)?.set("Contents", id);
        }
    }
    Ok(())
}

fn set_form_content(stream: &mut Stream, content: Vec<u8>) {
    let empty = paints_nothing(&content);
    stream.set_plain_content(if empty { Vec::new() } else { content });
    if empty {
        stream.dict.remove(b"Resources");
    } else {
        let _ = stream.compress();
    }
}

impl Applied {
    /// 페이지(0부터)의 현재 상태를 원본으로 기록한다(이미 있으면 그대로).
    pub fn record_page(&mut self, doc: &Document, index: usize, page_id: ObjectId) -> anyhow::Result<()> {
        if let std::collections::hash_map::Entry::Vacant(slot) = self.page_originals.entry(index) {
            slot.insert(doc.get_object(page_id)?.clone());
        }
        Ok(())
    }

    /// 기록된(바뀐) 페이지들(0부터).
    pub fn changed_pages(&self) -> BTreeSet<usize> {
        self.page_originals.keys().copied().collect()
    }

    /// 페이지들(0부터)을 원래대로 되돌린다. 제자리에서 고친 Form을 쓰던 페이지면 그 Form도
    /// 원본으로 되돌리며, 그 때문에 함께 원래대로 돌아간 다른 페이지 번호를 돌려준다.
    pub fn rollback(&mut self, doc: &mut Document, pages: &BTreeSet<usize>) -> BTreeSet<usize> {
        let mut also = BTreeSet::new();
        for page in pages {
            if let Some(original) = self.page_originals.remove(page) {
                if let Some(id) = page_object_id(doc, *page) {
                    doc.objects.insert(id, original);
                }
            }
        }
        let forms: Vec<ObjectId> = self
            .form_originals
            .iter()
            .filter(|(_, (_, users))| users.iter().any(|u| pages.contains(u)))
            .map(|(id, _)| *id)
            .collect();
        for form in forms {
            if let Some((original, users)) = self.form_originals.remove(&form) {
                doc.objects.insert(form, original);
                also.extend(users.into_iter().filter(|u| !pages.contains(u)));
            }
        }
        also
    }
}

fn page_object_id(doc: &Document, index: usize) -> Option<ObjectId> {
    doc.get_pages().get(&(index as u32 + 1)).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::interp::tests::{form, one_page_doc};
    use lopdf::dictionary;

    fn page_text(doc: &Document, page: ObjectId) -> String {
        String::from_utf8_lossy(&page_content_bytes(doc, page).unwrap()).to_string()
    }

    /// 폭을 아는 단순 폰트(/F1: 'A'~'B' 폭 500, 600)를 페이지 리소스에 넣는다.
    fn add_font(doc: &mut Document, page: ObjectId) {
        let font = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Test",
            "FirstChar" => 65, "Widths" => vec![500.into(), 600.into()],
        });
        let resources = doc.get_dictionary_mut(page).unwrap().get_mut(b"Resources").unwrap().as_dict_mut().unwrap();
        resources.set("Font", dictionary! { "F1" => font });
    }

    /// 지면을 덮는 이미지를 먼저 그린 뒤 `content`를 잇는다 — **실제 스캔 PDF와 같은 구성**.
    ///
    /// OCR 텍스트 판정이 "이미지 영역 안이거나 걸친 `3 Tr`"이므로(→ `classify::overlaps_image`),
    /// 이미지가 없으면 `3 Tr` 텍스트는 OCR로 잡히지 않는다. 지우기 기계(이동량 보존·상태 유지·
    /// Form 처리)를 시험하는 테스트들은 먼저 이 구성을 갖춰야 한다.
    fn scanned_page(content: &[u8], xobjects: Vec<(&str, Stream)>) -> (Document, ObjectId) {
        let image = Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Image" }, vec![]);
        let mut all = vec![("Im0", image)];
        all.extend(xobjects);
        let mut full = b"q 612 0 0 792 0 0 cm /Im0 Do Q ".to_vec();
        full.extend_from_slice(content);
        one_page_doc(&full, all)
    }

    fn run(content: &[u8]) -> (Document, ObjectId, RemovalPlan) {
        let (mut doc, page) = scanned_page(content, vec![]);
        add_font(&mut doc, page);
        let plan = plan(&doc);
        apply(&mut doc, &plan, &mut Applied::default()).unwrap();
        (doc, page, plan)
    }

    #[test]
    fn removes_only_show_operators_and_keeps_state() {
        let (doc, page, plan) = run(b"BT /F1 10 Tf 3 Tr 1 0 0 1 5 5 Tm (AB) Tj ET BT 0 Tr (A) Tj ET");
        assert_eq!(plan.totals().get(HiddenKind::InvisibleMode), 1);
        let text = page_text(&doc, page);
        assert!(text.contains("3 Tr 1 0 0 1 5 5 Tm  ET"), "{text}");
        assert!(text.contains("(A) Tj"));
    }

    #[test]
    fn following_visible_text_keeps_its_position() {
        // 지운 (AB) 뒤에 위치 재설정 없이 보이는 텍스트 — 이동량 11pt 보존: n = -11 / (10/1000) = -1100
        // (TJ 숫자는 음수일 때 오른쪽으로 옮긴다)
        let (doc, page, _) = run(b"BT /F1 10 Tf 3 Tr (AB) Tj 0 Tr (A) Tj ET");
        assert!(page_text(&doc, page).contains("[-1100] TJ"), "{}", page_text(&doc, page));
    }

    #[test]
    fn quote_operators_keep_line_moves() {
        let (doc, page, _) = run(b"BT /F1 10 Tf 12 TL 1 0 0 1 20 700 Tm 3 Tr (A) ' 1 2 (B) \" ET");
        let text = page_text(&doc, page);
        assert!(text.contains(" T* "), "{text}");
        assert!(text.contains(" 1 Tw 2 Tc T* "), "{text}");
    }

    #[test]
    fn only_invisible_mode_is_removed_others_reported() {
        let (_, _, plan) = run(b"BT /F1 0 Tf (A) Tj ET /Clear gs BT /F1 10 Tf (A) Tj ET BT 7 Tr (B) Tj ET");
        let (t, r) = (plan.totals(), plan.reported_totals());
        // 크기 0·투명·클리핑 모드는 안 보이기는 하지만 OCR 도구가 쓰는 기법이 아니다 —
        // 찾아만 두고 지우지 않는다(2026-09-28 결정).
        assert_eq!(t.removed(), 0, "3 Tr 외에는 지우지 않는다");
        for kind in [HiddenKind::ZeroSize, HiddenKind::Transparent, HiddenKind::ClipMode] {
            assert_eq!(r.get(kind), 1, "{kind:?}");
        }
    }

    /// **OCR 텍스트는 "이미지 영역 안이거나 걸친 `3 Tr`"이다**(2026-09-28 확정).
    ///
    /// 이미지와 무관한 자리의 `3 Tr`은 OCR이 만든 것이 아니므로(디지털 문서의 숨긴 값 등)
    /// 건드리지 않는다. 같은 `3 Tr`이라도 자리에 따라 판정이 갈린다는 것이 이 규칙의 요점이다.
    #[test]
    fn invisible_text_counts_as_ocr_only_where_a_scan_is() {
        let image = || Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Image" }, vec![]);
        // 이미지는 지면 왼쪽 아래 1/4에만 놓는다(0,0)-(306,396).
        // 글자 셋: 이미지 안 · 이미지에 걸침(경계 위) · 이미지 밖.
        let content: &[u8] = b"q 306 0 0 396 0 0 cm /Im0 Do Q \
             BT /F1 10 Tf 3 Tr 1 0 0 1 100 100 Tm (A) Tj ET \
             BT /F1 10 Tf 3 Tr 1 0 0 1 300 200 Tm (B) Tj ET \
             BT /F1 10 Tf 3 Tr 1 0 0 1 400 600 Tm (C) Tj ET";
        let (mut doc, page) = one_page_doc(content, vec![("Im0", image())]);
        add_font(&mut doc, page);
        let plan = plan(&doc);
        assert_eq!(plan.totals().get(HiddenKind::InvisibleMode), 2, "이미지 안·걸친 것만");
        apply(&mut doc, &plan, &mut Applied::default()).unwrap();
        let text = page_text(&doc, page);
        assert!(!text.contains("(A) Tj") && !text.contains("(B) Tj"), "{text}");
        assert!(text.contains("(C) Tj"), "이미지 밖 3 Tr은 남는다: {text}");
    }

    /// 이미지가 아예 없는 페이지의 `3 Tr`은 손대지 않는다 — 디지털 문서의 숨긴 텍스트.
    #[test]
    fn invisible_text_without_any_image_is_left_alone() {
        let (mut doc, page) = one_page_doc(b"BT /F1 10 Tf 3 Tr 1 0 0 1 100 100 Tm (A) Tj ET", vec![]);
        add_font(&mut doc, page);
        let plan = plan(&doc);
        assert_eq!(plan.totals().removed(), 0);
        assert!(!plan.has_changes());
        apply(&mut doc, &plan, &mut Applied::default()).unwrap();
        assert!(page_text(&doc, page).contains("(A) Tj"));
    }

    /// 적극 모드에서만 지우는 형태들 — 표준 모드에서는 보고만 한다.
    #[test]
    fn aggressive_only_kinds() {
        let image = || Stream::new(dictionary! { "Type" => "XObject", "Subtype" => "Image" }, vec![]);
        // 흰 글씨 / 페이지 밖 / 클리핑 밖 / 이미지 아래 — 각각 한 번씩.
        let content: &[u8] = b"BT /F1 10 Tf 1 1 1 rg 1 0 0 1 20 20 Tm (A) Tj ET             0 0 0 rg BT 1 0 0 1 900 900 Tm (A) Tj ET             q 0 0 10 10 re W n BT 1 0 0 1 300 300 Tm (A) Tj ET Q             BT 1 0 0 1 100 100 Tm (A) Tj ET q 612 0 0 792 0 0 cm /Im0 Do Q";
        let (mut doc, page) = one_page_doc(content, vec![("Im0", image())]);
        add_font(&mut doc, page);
        let standard = plan(&doc);
        assert_eq!(standard.totals().removed(), 0);
        let reported = standard.reported_totals();
        for kind in [HiddenKind::WhiteText, HiddenKind::OutsidePage, HiddenKind::Clipped, HiddenKind::UnderImage] {
            assert_eq!(reported.get(kind), 1, "{kind:?}");
        }
        let aggressive = plan_for(&doc, None, Mode::Aggressive);
        assert_eq!(aggressive.totals().removed(), 4);
        apply(&mut doc, &aggressive, &mut Applied::default()).unwrap();
        assert!(!page_text(&doc, page).contains("Tj"), "{}", page_text(&doc, page));
    }

    /// 기본으로 꺼진 레이어(OCG) 안의 텍스트 — 마크드 콘텐츠와 Form의 `/OC` 양쪽.
    #[test]
    fn hidden_layer_text() {
        let (mut doc, page) = one_page_doc(b"/OC /L1 BDC BT /F1 10 Tf (A) Tj ET EMC /X0 Do", vec![("X0", form(b"BT /F1 10 Tf (B) Tj ET"))]);
        add_font(&mut doc, page);
        let ocg = doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("OCR") });
        let properties = dictionary! { "OCGs" => vec![ocg.into()], "D" => dictionary! { "OFF" => vec![ocg.into()] } };
        let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        doc.get_dictionary_mut(root).unwrap().set("OCProperties", properties);
        // 페이지 리소스의 /Properties에 레이어를 걸고, Form에는 /OC를 붙인다.
        let form_id = {
            let resources = doc.get_dictionary_mut(page).unwrap().get_mut(b"Resources").unwrap().as_dict_mut().unwrap();
            resources.set("Properties", dictionary! { "L1" => ocg });
            resources.get(b"XObject").unwrap().as_dict().unwrap().get(b"X0").unwrap().as_reference().unwrap()
        };
        doc.get_object_mut(form_id).unwrap().as_stream_mut().unwrap().dict.set("OC", ocg);

        assert_eq!(plan(&doc).reported_totals().get(HiddenKind::HiddenLayer), 2);
        let aggressive = plan_for(&doc, None, Mode::Aggressive);
        assert_eq!(aggressive.totals().get(HiddenKind::HiddenLayer), 2);
    }

    /// 꺼진 레이어의 글자를 지우면 빈 `BDC`·`EMC` 껍데기와 레이어 목록도 정리된다.
    #[test]
    fn empty_hidden_layer_is_cleaned_up() {
        let (mut doc, page) = one_page_doc(b"/OC /L1 BDC BT /F1 10 Tf (A) Tj ET EMC BT /F1 10 Tf 1 0 0 1 0 50 Tm (B) Tj ET", vec![]);
        add_font(&mut doc, page);
        let ocg = doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("OCR") });
        let root = doc.trailer.get(b"Root").unwrap().as_reference().unwrap();
        doc.get_dictionary_mut(root).unwrap().set(
            "OCProperties",
            dictionary! { "OCGs" => vec![ocg.into()], "D" => dictionary! { "OFF" => vec![ocg.into()], "Order" => vec![ocg.into()] } },
        );
        doc.get_dictionary_mut(page)
            .unwrap()
            .get_mut(b"Resources")
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Properties", dictionary! { "L1" => ocg });

        let plan = plan_for(&doc, None, Mode::Aggressive);
        apply(&mut doc, &plan, &mut Applied::default()).unwrap();
        let text = page_text(&doc, page);
        assert!(!text.contains("BDC") && !text.contains("EMC"), "빈 레이어 껍데기가 남음: {text}");
        assert!(text.contains("(B) Tj"), "레이어 밖 글자는 남는다: {text}");
        assert_eq!(prune_optional_content(&mut doc), 1);
        let properties = doc.get_dictionary(root).unwrap().get(b"OCProperties").unwrap().as_dict().unwrap();
        assert!(properties.get(b"OCGs").unwrap().as_array().unwrap().is_empty());
    }

    /// 되돌리기는 저장 때 정리된 원본 스트림을 다시 가리킨다 — 정리한 객체를 보관했다가
    /// 되살려야 한다(실제로 이 순서를 빼먹어 되돌린 페이지가 빈 페이지가 됐다, 2026-09-23).
    #[test]
    fn rollback_after_pruning_restores_original_content() {
        let (mut doc, page) = scanned_page(b"BT /F1 10 Tf 3 Tr (A) Tj ET", vec![]);
        add_font(&mut doc, page);
        let original = page_text(&doc, page);
        let plan = plan(&doc);
        let mut applied = Applied::default();
        apply(&mut doc, &plan, &mut applied).unwrap();
        let dropped = crate::save::take_unreferenced(&mut doc);
        assert!(!dropped.is_empty(), "지운 뒤엔 원본 콘텐츠 스트림이 쓰이지 않는다");
        crate::save::restore(&mut doc, dropped);
        applied.rollback(&mut doc, &BTreeSet::from([0]));
        assert_eq!(page_text(&doc, page), original);
    }

    /// 태그된 마크드 콘텐츠의 글자를 지우면 "빈 태그"로 센다(껍데기는 남겨 구조 트리는 그대로).
    #[test]
    fn empty_tags_are_counted_not_removed() {
        let (mut doc, page) = scanned_page(
            // 둘째 블록에서 0 Tr로 되돌린다 — 텍스트 상태는 ET 뒤에도 유지되기 때문.
            b"/P <</MCID 0>> BDC BT /F1 10 Tf 3 Tr (A) Tj ET EMC /P <</MCID 1>> BDC BT 0 Tr (B) Tj ET EMC",
            vec![],
        );
        add_font(&mut doc, page);
        let plan = plan(&doc);
        assert_eq!(plan.empty_tags(), 1, "지운 쪽 태그만 빈 것으로 센다");
        apply(&mut doc, &plan, &mut Applied::default()).unwrap();
        let text = page_text(&doc, page);
        assert_eq!(text.matches("BDC").count(), 2, "태그 껍데기는 남긴다: {text}");
        assert!(text.contains("(B) Tj"));
    }

    #[test]
    fn unknown_width_with_following_text_skips_page() {
        let (mut doc, page) = scanned_page(b"BT /F9 10 Tf 3 Tr (A) Tj 0 Tr (B) Tj ET", vec![]);
        let plan = plan(&doc);
        assert!(matches!(plan.pages[0].status, PageStatus::Skipped(_)));
        apply(&mut doc, &plan, &mut Applied::default()).unwrap();
        assert!(page_text(&doc, page).contains("(A) Tj"));
    }

    #[test]
    fn ocr_form_is_emptied_in_place() {
        let (mut doc, page) = scanned_page(b"q /X0 Do Q", vec![("X0", form(b"BT 3 Tr (x) Tj ET"))]);
        let form_id = resource_entry(&doc, &crate::content::interp::page_resources(&doc, page), b"XObject", b"X0")
            .unwrap()
            .as_reference()
            .unwrap();
        let plan = plan(&doc);
        assert!(plan.has_changes());
        let mut applied = Applied::default();
        apply(&mut doc, &plan, &mut applied).unwrap();
        let stream = doc.get_object(form_id).unwrap().as_stream().unwrap();
        assert!(stream.content.is_empty() && !stream.dict.has(b"Resources"));
        // 되돌리기
        applied.rollback(&mut doc, &BTreeSet::from([0]));
        assert_eq!(stream_bytes(&doc, form_id).unwrap(), b"BT 3 Tr (x) Tj ET");
    }

    #[test]
    fn form_used_with_different_inherited_state_is_cloned_per_page() {
        // 같은 Form을 한 번은 3 Tr 아래, 한 번은 보이게 부른다 → 결과가 달라 제자리 수정 불가.
        let (mut doc, page) = scanned_page(b"3 Tr /X0 Do", vec![("X0", form(b"BT (x) Tj ET"))]);
        let form_id = resource_entry(&doc, &crate::content::interp::page_resources(&doc, page), b"XObject", b"X0")
            .unwrap()
            .as_reference()
            .unwrap();
        // 다른 곳(주석 외형을 흉내 낸 딕셔너리)에서도 참조 — 해석하지 않은 사용처.
        doc.add_object(dictionary! { "AP" => form_id });
        let plan = plan(&doc);
        apply(&mut doc, &plan, &mut Applied::default()).unwrap();
        assert_eq!(stream_bytes(&doc, form_id).unwrap(), b"BT (x) Tj ET", "원본은 그대로");
        let new_ref = resource_entry(&doc, &crate::content::interp::page_resources(&doc, page), b"XObject", b"X0")
            .unwrap()
            .as_reference()
            .unwrap();
        assert_ne!(new_ref, form_id);
    }
}
