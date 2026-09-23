//! 선택적 콘텐츠(레이어, OCG·OCMD) — 기본 상태에서 숨겨진 레이어 판정과 정리(설계 문서 3장 F, 4.4).
//!
//! OCR을 레이어로 넣는 도구가 있고, 그 레이어가 기본으로 꺼져 있으면 화면에 보이지 않는다. 다만
//! 인쇄용·언어별 레이어처럼 정상 콘텐츠일 수도 있어서, 삭제는 적극 모드에서 레이어 이름을 보여 준 뒤
//! 렌더 검증을 통과할 때만 한다.
//!
//! 판정 기준은 `/OCProperties /D`(기본 설정)다. `/BaseState`가 `/OFF`면 `/ON`에 든 것만 보이고,
//! 그 밖에는 `/OFF`에 든 것만 숨는다. OCMD는 `/OCGs`와 `/P`(AnyOn·AllOn·AnyOff·AllOff)로 따지고,
//! 가시성 식(`/VE`)이 있으면 판정하지 않는다(모름).

use crate::geometry::resolve;
use lopdf::{Document, Object, ObjectId};
use std::collections::HashSet;

#[derive(Debug, Default, Clone)]
pub struct OptionalContent {
    /// 기본 설정에서 꺼져 있는 OCG.
    off: HashSet<ObjectId>,
    /// 문서가 선언한 모든 OCG.
    all: HashSet<ObjectId>,
}

impl OptionalContent {
    pub fn load(doc: &Document) -> Self {
        let mut content = Self::default();
        let Some(properties) = doc.catalog().ok().and_then(|c| c.get(b"OCProperties").ok()).and_then(|o| resolve(doc, o).as_dict().ok())
        else {
            return content;
        };
        let ids = |key: &[u8], dict: &lopdf::Dictionary| -> Vec<ObjectId> {
            dict.get(key)
                .ok()
                .and_then(|o| resolve(doc, o).as_array().ok())
                .map(|a| a.iter().filter_map(|o| o.as_reference().ok()).collect())
                .unwrap_or_default()
        };
        content.all = ids(b"OCGs", properties).into_iter().collect();
        let Some(default) = properties.get(b"D").ok().and_then(|o| resolve(doc, o).as_dict().ok()) else {
            return content;
        };
        let base_off = default.get(b"BaseState").ok().and_then(|o| o.as_name().ok()) == Some(b"OFF");
        if base_off {
            let on: HashSet<ObjectId> = ids(b"ON", default).into_iter().collect();
            content.off = content.all.difference(&on).copied().collect();
        } else {
            content.off = ids(b"OFF", default).into_iter().collect();
        }
        content
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    /// `id`(OCG 또는 OCMD)가 기본 설정에서 숨겨지는지. 판정할 수 없으면 None.
    pub fn is_hidden(&self, doc: &Document, id: ObjectId) -> Option<bool> {
        let dict = doc.get_dictionary(id).ok()?;
        if dict.get(b"Type").ok().and_then(|o| o.as_name().ok()) != Some(b"OCMD") {
            return Some(self.off.contains(&id));
        }
        if dict.has(b"VE") {
            return None; // 가시성 식 — 평가하지 않는다
        }
        let members: Vec<ObjectId> = match dict.get(b"OCGs").ok().map(|o| resolve(doc, o)) {
            Some(Object::Array(items)) => items.iter().filter_map(|o| o.as_reference().ok()).collect(),
            _ => dict.get(b"OCGs").ok().and_then(|o| o.as_reference().ok()).into_iter().collect(),
        };
        if members.is_empty() {
            return Some(false);
        }
        let visible: Vec<bool> = members.iter().map(|m| !self.off.contains(m)).collect();
        let policy = dict.get(b"P").ok().and_then(|o| o.as_name().ok()).unwrap_or(b"AnyOn");
        let shown = match policy {
            b"AllOn" => visible.iter().all(|v| *v),
            b"AnyOff" => visible.iter().any(|v| !*v),
            b"AllOff" => visible.iter().all(|v| !*v),
            _ => visible.iter().any(|v| *v), // AnyOn
        };
        Some(!shown)
    }

    /// 레이어 이름(리포트·확인 창에 보여 줄 것).
    pub fn name(&self, doc: &Document, id: ObjectId) -> Option<String> {
        let dict = doc.get_dictionary(id).ok()?;
        let name = dict.get(b"Name").ok()?;
        Some(crate::hocr::write::escape(&String::from_utf8_lossy(name.as_str().ok()?)))
    }
}

/// `referenced`에 없는 OCG를 `/OCProperties`의 목록(`/OCGs`, `/D`의 `/ON`·`/OFF`·`/Order`)에서 뺀다.
/// 남겨 두면 뷰어 레이어 패널에 빈 레이어가 보인다. 지운 개수를 돌려준다.
pub fn prune_unreferenced(doc: &mut Document, referenced: &HashSet<ObjectId>) -> usize {
    let Ok(catalog) = doc.catalog() else { return 0 };
    let Some(properties_id) = catalog.get(b"OCProperties").ok().and_then(|o| o.as_reference().ok()) else {
        // 카탈로그에 직접 들어 있는 경우도 처리한다.
        return prune_in_place(doc, referenced);
    };
    let Ok(properties) = doc.get_object_mut(properties_id).and_then(Object::as_dict_mut) else {
        return 0;
    };
    prune_dictionary(properties, referenced)
}

fn prune_in_place(doc: &mut Document, referenced: &HashSet<ObjectId>) -> usize {
    let Ok(root) = doc.trailer.get(b"Root").and_then(Object::as_reference) else {
        return 0;
    };
    let Ok(catalog) = doc.get_object_mut(root).and_then(Object::as_dict_mut) else {
        return 0;
    };
    let Ok(properties) = catalog.get_mut(b"OCProperties").and_then(Object::as_dict_mut) else {
        return 0;
    };
    prune_dictionary(properties, referenced)
}

fn prune_dictionary(properties: &mut lopdf::Dictionary, referenced: &HashSet<ObjectId>) -> usize {
    let mut removed = 0;
    if let Ok(Object::Array(list)) = properties.get_mut(b"OCGs") {
        removed = prune_array(list, referenced);
    }
    if let Ok(Object::Dictionary(default)) = properties.get_mut(b"D") {
        for key in [&b"ON"[..], b"OFF", b"Order", b"AS", b"Locked"] {
            if let Ok(Object::Array(list)) = default.get_mut(key) {
                prune_array(list, referenced);
            }
        }
    }
    removed
}

/// 배열(중첩 포함)에서 쓰이지 않는 OCG 참조를 뺀다. 지운 개수.
fn prune_array(list: &mut Vec<Object>, referenced: &HashSet<ObjectId>) -> usize {
    let mut removed = 0;
    list.retain_mut(|item| match item {
        Object::Reference(id) => {
            let keep = referenced.contains(id);
            removed += usize::from(!keep);
            keep
        }
        Object::Array(inner) => {
            removed += prune_array(inner, referenced);
            !inner.is_empty()
        }
        _ => true,
    });
    removed
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    fn doc_with_layers() -> (Document, ObjectId, ObjectId, ObjectId) {
        let mut doc = Document::with_version("1.7");
        let visible = doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("본문") });
        let hidden = doc.add_object(dictionary! { "Type" => "OCG", "Name" => Object::string_literal("OCR") });
        let properties = dictionary! {
            "OCGs" => vec![visible.into(), hidden.into()],
            "D" => dictionary! { "OFF" => vec![hidden.into()], "Order" => vec![visible.into(), hidden.into()] },
        };
        let pages = doc.add_object(dictionary! { "Type" => "Pages", "Kids" => vec![], "Count" => 0 });
        let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages, "OCProperties" => properties });
        doc.trailer.set("Root", catalog);
        (doc, visible, hidden, catalog)
    }

    #[test]
    fn hidden_layers_and_ocmd_policies() {
        let (mut doc, visible, hidden, _) = doc_with_layers();
        let oc = OptionalContent::load(&doc);
        assert_eq!(oc.is_hidden(&doc, hidden), Some(true));
        assert_eq!(oc.is_hidden(&doc, visible), Some(false));
        assert_eq!(oc.name(&doc, hidden).as_deref(), Some("OCR"));

        let any_on = doc.add_object(dictionary! { "Type" => "OCMD", "OCGs" => vec![visible.into(), hidden.into()] });
        let all_on = doc.add_object(dictionary! { "Type" => "OCMD", "OCGs" => vec![visible.into(), hidden.into()], "P" => "AllOn" });
        let any_off = doc.add_object(dictionary! { "Type" => "OCMD", "OCGs" => vec![visible.into()], "P" => "AnyOff" });
        let expression = doc.add_object(dictionary! { "Type" => "OCMD", "VE" => vec![] });
        assert_eq!(oc.is_hidden(&doc, any_on), Some(false));
        assert_eq!(oc.is_hidden(&doc, all_on), Some(true));
        assert_eq!(oc.is_hidden(&doc, any_off), Some(true));
        assert_eq!(oc.is_hidden(&doc, expression), None);
    }

    #[test]
    fn base_state_off_hides_everything_not_listed() {
        let (mut doc, visible, hidden, catalog) = doc_with_layers();
        let properties = dictionary! {
            "OCGs" => vec![visible.into(), hidden.into()],
            "D" => dictionary! { "BaseState" => "OFF", "ON" => vec![visible.into()] },
        };
        doc.get_dictionary_mut(catalog).unwrap().set("OCProperties", properties);
        let oc = OptionalContent::load(&doc);
        assert_eq!((oc.is_hidden(&doc, visible), oc.is_hidden(&doc, hidden)), (Some(false), Some(true)));
    }

    #[test]
    fn pruning_removes_unreferenced_layers() {
        let (mut doc, visible, hidden, catalog) = doc_with_layers();
        let removed = prune_unreferenced(&mut doc, &HashSet::from([visible]));
        assert_eq!(removed, 1);
        let properties = doc.get_dictionary(catalog).unwrap().get(b"OCProperties").unwrap().as_dict().unwrap();
        assert_eq!(properties.get(b"OCGs").unwrap().as_array().unwrap().len(), 1);
        let order = properties.get(b"D").unwrap().as_dict().unwrap().get(b"Order").unwrap().as_array().unwrap();
        assert_eq!(order.len(), 1);
        assert!(!order.contains(&Object::Reference(hidden)));
    }
}
