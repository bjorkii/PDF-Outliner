//! 공유 객체 판별과 clone-on-write(설계 문서 2.3).
//!
//! 콘텐츠 스트림, Form XObject, Resources 딕셔너리는 여러 페이지가 함께 쓰는 경우가 흔하다
//! (머리글·워터마크 XObject, Pages 트리에서 상속한 Resources). 한 페이지를 고치려고 공유 객체를
//! 그대로 수정하면 다른 페이지까지 바뀐다. 그래서 문서 전체의 참조 횟수를 먼저 세고, 공유된
//! 객체는 복제한 뒤 이 페이지(또는 이 Form)의 참조만 복제본으로 바꾼다.

use crate::geometry::{inherited, resolve};
use anyhow::{Context, Result};
use lopdf::{Dictionary, Document, Object, ObjectId};
use std::collections::HashMap;

/// 객체 id별 간접 참조 횟수(트레일러 포함).
#[derive(Debug, Default, Clone)]
pub struct ReferenceCounts(HashMap<ObjectId, usize>);

impl ReferenceCounts {
    pub fn count(doc: &Document) -> Self {
        let mut counts = HashMap::new();
        for object in doc.objects.values() {
            collect_references(object, &mut counts);
        }
        for (_, value) in doc.trailer.iter() {
            collect_references(value, &mut counts);
        }
        Self(counts)
    }

    pub fn get(&self, id: ObjectId) -> usize {
        self.0.get(&id).copied().unwrap_or(0)
    }

    pub fn is_shared(&self, id: ObjectId) -> bool {
        self.get(id) > 1
    }

    fn add_references_in(&mut self, object: &Object) {
        collect_references(object, &mut self.0);
    }

    fn decrement(&mut self, id: ObjectId) {
        if let Some(count) = self.0.get_mut(&id) {
            *count = count.saturating_sub(1);
        }
    }
}

fn collect_references(object: &Object, counts: &mut HashMap<ObjectId, usize>) {
    match object {
        Object::Reference(id) => *counts.entry(*id).or_default() += 1,
        Object::Array(items) => items.iter().for_each(|o| collect_references(o, counts)),
        Object::Dictionary(dict) => dict.iter().for_each(|(_, o)| collect_references(o, counts)),
        Object::Stream(stream) => stream.dict.iter().for_each(|(_, o)| collect_references(o, counts)),
        _ => {}
    }
}

/// 참조 하나(`id`)를 고치기 전에 부른다. 공유된 객체면 복제본을 만들어 그 id를 돌려주고
/// (호출 측이 자기 참조를 새 id로 바꿔야 함), 공유되지 않았으면 원래 id를 그대로 돌려준다.
/// 복제본이 품은 참조들의 횟수도 함께 늘려서 이후 공유 판별이 계속 맞게 한다.
pub fn make_private(doc: &mut Document, counts: &mut ReferenceCounts, id: ObjectId) -> Result<ObjectId> {
    if !counts.is_shared(id) {
        return Ok(id);
    }
    let copy = doc.get_object(id).with_context(|| format!("객체 {id:?} 조회 실패"))?.clone();
    counts.add_references_in(&copy);
    let new_id = doc.add_object(copy);
    counts.decrement(id);
    counts.0.insert(new_id, 1);
    Ok(new_id)
}

/// 페이지가 자기 전용 Resources 딕셔너리를 직접 갖게 한다. 상속받은 Resources는 복사해서 페이지에
/// 넣고(상위 Pages 노드는 고치지 않음), 공유된 간접 Resources는 복제본으로 바꾼다. 이후
/// `page_resources_mut`로 안전하게 고칠 수 있다.
pub fn ensure_private_page_resources(doc: &mut Document, counts: &mut ReferenceCounts, page_id: ObjectId) -> Result<()> {
    let page = doc.get_dictionary(page_id).context("페이지 딕셔너리 조회 실패")?;
    match page.get(b"Resources") {
        Ok(Object::Dictionary(_)) => {}
        Ok(Object::Reference(id)) => {
            let id = *id;
            let private = make_private(doc, counts, id)?;
            // 간접 딕셔너리는 페이지 안으로 옮긴다 — 이후 편집이 한 곳에서 끝난다.
            let dict = doc.get_dictionary(private).cloned().unwrap_or_default();
            counts.decrement(private);
            doc.get_dictionary_mut(page_id)?.set("Resources", dict);
        }
        _ => {
            let inherited_dict = inherited(doc, page, b"Resources")
                .and_then(|o| resolve(doc, o).as_dict().ok())
                .cloned()
                .unwrap_or_default();
            counts.add_references_in(&Object::Dictionary(inherited_dict.clone()));
            doc.get_dictionary_mut(page_id)?.set("Resources", inherited_dict);
        }
    }
    Ok(())
}

/// 페이지 전용 Resources 안의 하위 딕셔너리(`/XObject`, `/Font` 등)를 고칠 수 있게 직접 값으로
/// 만든다(없으면 새로 만든다). `ensure_private_page_resources`를 먼저 불러야 한다.
pub fn page_subdict_mut<'a>(
    doc: &'a mut Document,
    counts: &mut ReferenceCounts,
    page_id: ObjectId,
    category: &[u8],
) -> Result<&'a mut Dictionary> {
    let current = doc
        .get_dictionary(page_id)?
        .get(b"Resources")
        .and_then(Object::as_dict)
        .context("페이지 전용 Resources가 없음 — ensure_private_page_resources를 먼저 호출")?
        .get(category)
        .ok()
        .cloned();
    let sub = match current {
        Some(Object::Dictionary(dict)) => dict,
        Some(Object::Reference(id)) => {
            // 공유 여부와 관계없이 값으로 옮긴다(원본 간접 객체는 다른 곳이 계속 쓴다).
            let dict = doc.get_dictionary(id).cloned().unwrap_or_default();
            counts.decrement(id);
            counts.add_references_in(&Object::Dictionary(dict.clone()));
            dict
        }
        _ => Dictionary::new(),
    };
    let resources = doc
        .get_dictionary_mut(page_id)?
        .get_mut(b"Resources")?
        .as_dict_mut()?;
    resources.set(category.to_vec(), sub);
    Ok(resources.get_mut(category)?.as_dict_mut()?)
}

/// `dict`에 없는 새 리소스 이름(`prefix` + 번호).
pub fn unique_name(dict: &Dictionary, prefix: &str) -> Vec<u8> {
    (0..)
        .map(|n| format!("{prefix}{n}").into_bytes())
        .find(|name| !dict.has(name))
        .expect("무한 반복자")
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::dictionary;

    /// 두 페이지가 Pages 노드의 Resources를 상속하고, 같은 XObject를 공유한다.
    fn two_pages_sharing() -> (Document, ObjectId, ObjectId, ObjectId) {
        let mut doc = Document::with_version("1.7");
        let pages_id = doc.new_object_id();
        let xobject = doc.add_object(dictionary! { "Subtype" => "Form" });
        let resources = doc.add_object(dictionary! { "XObject" => dictionary! { "X0" => xobject } });
        let p1 = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages_id });
        let p2 = doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages_id });
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => vec![p1.into(), p2.into()], "Count" => 2, "Resources" => resources,
            }),
        );
        (doc, p1, p2, xobject)
    }

    #[test]
    fn inherited_resources_are_copied_into_page_only() {
        let (mut doc, p1, p2, xobject) = two_pages_sharing();
        let mut counts = ReferenceCounts::count(&doc);
        assert_eq!(counts.get(xobject), 1); // 공유 Resources 안에 한 번
        ensure_private_page_resources(&mut doc, &mut counts, p1).unwrap();
        let sub = page_subdict_mut(&mut doc, &mut counts, p1, b"XObject").unwrap();
        let name = unique_name(sub, "X");
        assert_eq!(name, b"X1");
        sub.set(name, Object::Null);
        // 페이지 1만 바뀌고 페이지 2는 여전히 상속.
        assert!(doc.get_dictionary(p2).unwrap().get(b"Resources").is_err());
        // 복사된 Resources가 XObject를 참조하므로 이제 공유 상태.
        assert!(counts.is_shared(xobject));
        let fresh = ReferenceCounts::count(&doc);
        assert_eq!(fresh.get(xobject), counts.get(xobject));
    }

    #[test]
    fn shared_object_is_cloned_once() {
        let (mut doc, _p1, _p2, xobject) = two_pages_sharing();
        let mut counts = ReferenceCounts::count(&doc);
        assert_eq!(make_private(&mut doc, &mut counts, xobject).unwrap(), xobject); // 1회 참조
        let holder = doc.add_object(dictionary! { "Again" => xobject });
        let _ = holder;
        let mut counts = ReferenceCounts::count(&doc);
        let copy = make_private(&mut doc, &mut counts, xobject).unwrap();
        assert_ne!(copy, xobject);
        assert_eq!(counts.get(xobject), 1);
        assert_eq!(counts.get(copy), 1);
    }
}
