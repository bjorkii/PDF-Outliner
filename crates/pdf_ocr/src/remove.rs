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

use crate::classify::{classify, is_show_operator, HiddenKind, Verdict};
use crate::content::interp::{interpret_page, resource_entry, Context, GraphicsState, Problem, Source, Visitor, XObjectKind};
use crate::content::lexer::{tokenize, Operation};
use crate::content::{page_content_bytes, stream_bytes};
use crate::fonts::{tj_number_for_advance, FontMetrics};
use crate::resources::{ensure_private_page_resources, page_subdict_mut, ReferenceCounts};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KindCounts {
    pub invisible_mode: usize,
    pub zero_size: usize,
    pub transparent: usize,
    /// render mode 7 — 남겨 둔 것.
    pub clip_only_kept: usize,
}

impl KindCounts {
    pub fn removed(&self) -> usize {
        self.invisible_mode + self.zero_size + self.transparent
    }

    pub fn add(&mut self, other: &KindCounts) {
        self.invisible_mode += other.invisible_mode;
        self.zero_size += other.zero_size;
        self.transparent += other.transparent;
        self.clip_only_kept += other.clip_only_kept;
    }

    fn count(&mut self, kind: HiddenKind) {
        match kind {
            HiddenKind::InvisibleMode => self.invisible_mode += 1,
            HiddenKind::ZeroSize => self.zero_size += 1,
            HiddenKind::Transparent => self.transparent += 1,
        }
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
    pub counts: KindCounts,
    pub status: PageStatus,
    /// 처리는 했지만 알려야 할 것(해석하지 못한 Form 등).
    pub notes: Vec<String>,
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

    pub fn has_changes(&self) -> bool {
        self.pages.iter().any(|p| p.status == PageStatus::Planned)
    }
}

// ------------------------------------------------------------------ 해석·분류

#[derive(Debug, Clone, Copy)]
struct Removal {
    /// 지운 표시 연산자의 이동량(텍스트 공간, Th 적용 후). 계산하지 못하면 None.
    advance: Option<f64>,
    font_size: f64,
    horizontal_scaling: f64,
}

#[derive(Default)]
struct StreamVisit {
    removals: BTreeMap<usize, Removal>,
    counts: KindCounts,
    problem: Option<String>,
}

struct Frame {
    form: Option<(ObjectId, usize, Vec<u8>)>,
    visit: StreamVisit,
}

struct Collector<'a> {
    doc: &'a Document,
    stack: Vec<Frame>,
    pending_name: Vec<u8>,
    finished_forms: Vec<(ObjectId, usize, Vec<u8>, StreamVisit)>,
    fonts: HashMap<ObjectId, Option<FontMetrics>>,
}

impl Collector<'_> {
    fn advance(&mut self, context: &Context, op: &Operation) -> Option<f64> {
        let text = &context.state.text;
        let (id, direct) = match (&text.font_name, text.font_ref) {
            (_, Some(id)) => (Some(id), None),
            (Some(name), None) => {
                let entry = resource_entry(self.doc, context.resources, b"Font", name)?;
                match entry {
                    Object::Reference(id) => (Some(*id), None),
                    other => (None, other.as_dict().ok()),
                }
            }
            (None, None) => return None,
        };
        let metrics = match (id, direct) {
            (Some(id), _) => {
                let doc = self.doc;
                self.fonts
                    .entry(id)
                    .or_insert_with(|| doc.get_dictionary(id).ok().and_then(|d| FontMetrics::load(doc, d)))
                    .clone()?
            }
            (None, Some(dict)) => FontMetrics::load(self.doc, dict)?,
            _ => return None,
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
}

impl Visitor for Collector<'_> {
    fn operation(&mut self, context: &Context, index: usize, op: &Operation) {
        match classify(context, op) {
            Verdict::Keep => {}
            Verdict::ClipOnlyKept => {
                if let Some(frame) = self.stack.last_mut() {
                    frame.visit.counts.clip_only_kept += 1;
                }
            }
            Verdict::Remove(kind) => {
                let advance = self.advance(context, op);
                let removal = Removal {
                    advance,
                    font_size: context.state.text.font_size,
                    horizontal_scaling: context.state.text.horizontal_scaling,
                };
                if let Some(frame) = self.stack.last_mut() {
                    frame.visit.counts.count(kind);
                    frame.visit.removals.insert(index, removal);
                }
            }
        }
    }

    fn xobject(&mut self, _context: &Context, name: &[u8], _id: Option<ObjectId>, _kind: &XObjectKind) {
        self.pending_name = name.to_vec();
    }

    fn enter_form(&mut self, id: ObjectId, _state: &GraphicsState) -> bool {
        let depth = self.stack.len();
        let name = std::mem::take(&mut self.pending_name);
        self.stack.push(Frame { form: Some((id, depth, name)), visit: StreamVisit::default() });
        true
    }

    fn exit_form(&mut self, _id: ObjectId) {
        if let Some(Frame { form: Some((id, depth, name)), visit }) = self.stack.pop() {
            self.finished_forms.push((id, depth, name, visit));
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
        if let (Source::Form(id), Some(frame)) = (source, self.stack.last_mut()) {
            if frame.form.as_ref().is_some_and(|(fid, _, _)| *fid == id) {
                frame.visit.problem = Some(message);
                frame.visit.removals.clear();
                frame.visit.counts = KindCounts::default();
                return;
            }
        }
        if let Some(frame) = self.stack.last_mut() {
            let note = format!("Form {:?}: {message}", match source {
                Source::Form(id) | Source::Page(id) => id,
            });
            frame.visit.problem.get_or_insert(note);
        }
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
            let advance = removal.advance.ok_or("글자 폭을 계산할 수 없는 폰트(지운 텍스트 뒤에 같은 줄 텍스트가 이어짐)")?;
            let n = tj_number_for_advance(advance, removal.font_size, removal.horizontal_scaling)
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
    plan_for(doc, None)
}

/// `only`에 든 페이지(0부터)만 지우는 계획. 다른 페이지도 해석은 한다 — 공유 Form을 제자리에서
/// 고쳐도 되는지 판단하려면 모든 사용처를 알아야 하기 때문이다(그 페이지들의 사용처는 "그대로"로 친다).
pub fn plan_for(doc: &Document, only: Option<&BTreeSet<usize>>) -> RemovalPlan {
    let counts = ReferenceCounts::count(doc);
    let page_ids: Vec<ObjectId> = doc.get_pages().into_values().collect();
    let mut pages = Vec::with_capacity(page_ids.len());
    let mut visits: Vec<FormVisit> = Vec::new();

    for (page_index, &page_id) in page_ids.iter().enumerate() {
        let mut page = PagePlan {
            number: page_index + 1,
            page_id,
            counts: KindCounts::default(),
            status: PageStatus::Unchanged,
            notes: Vec::new(),
            content: None,
        };
        let mut collector = Collector {
            doc,
            stack: vec![Frame { form: None, visit: StreamVisit::default() }],
            pending_name: Vec::new(),
            finished_forms: Vec::new(),
            fonts: HashMap::new(),
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
        let page_visit = collector.stack.pop().map(|f| f.visit).unwrap_or_default();
        let mut skip: Option<String> = None;
        page.counts.add(&page_visit.counts);
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
        for (form, depth, name, visit) in collector.finished_forms {
            page.counts.add(&visit.counts);
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

    fn run(content: &[u8]) -> (Document, ObjectId, RemovalPlan) {
        let (mut doc, page) = one_page_doc(content, vec![]);
        add_font(&mut doc, page);
        let plan = plan(&doc);
        apply(&mut doc, &plan, &mut Applied::default()).unwrap();
        (doc, page, plan)
    }

    #[test]
    fn removes_only_show_operators_and_keeps_state() {
        let (doc, page, plan) = run(b"BT /F1 10 Tf 3 Tr 1 0 0 1 5 5 Tm (AB) Tj ET BT 0 Tr (A) Tj ET");
        assert_eq!(plan.totals().invisible_mode, 1);
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
        let (doc, page, _) = run(b"BT /F1 10 Tf 12 TL 3 Tr (A) ' 1 2 (B) \" ET");
        let text = page_text(&doc, page);
        assert!(text.contains(" T* "), "{text}");
        assert!(text.contains(" 1 Tw 2 Tc T* "), "{text}");
    }

    #[test]
    fn transparent_and_zero_size_and_clip_mode() {
        let (_, _, plan) = run(b"BT /F1 0 Tf (A) Tj ET /Clear gs BT /F1 10 Tf (A) Tj ET BT 7 Tr (B) Tj ET");
        let t = plan.totals();
        assert_eq!((t.zero_size, t.transparent, t.clip_only_kept), (1, 1, 1));
    }

    #[test]
    fn unknown_width_with_following_text_skips_page() {
        let (mut doc, page) = one_page_doc(b"BT /F9 10 Tf 3 Tr (A) Tj 0 Tr (B) Tj ET", vec![]);
        let plan = plan(&doc);
        assert!(matches!(plan.pages[0].status, PageStatus::Skipped(_)));
        apply(&mut doc, &plan, &mut Applied::default()).unwrap();
        assert!(page_text(&doc, page).contains("(A) Tj"));
    }

    #[test]
    fn ocr_form_is_emptied_in_place() {
        let (mut doc, page) = one_page_doc(b"q /X0 Do Q", vec![("X0", form(b"BT 3 Tr (x) Tj ET"))]);
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
        let (mut doc, page) = one_page_doc(b"3 Tr /X0 Do", vec![("X0", form(b"BT (x) Tj ET"))]);
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
