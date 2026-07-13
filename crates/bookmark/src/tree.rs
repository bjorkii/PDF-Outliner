use crate::model::{BookmarkNode, BookmarkRow};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum TreeError {
    #[error("대상 노드를 찾을 수 없음: {0}")]
    TargetNotFound(Uuid),
    #[error("자기 자신 또는 자신의 하위 트리로는 이동할 수 없음")]
    InvalidMoveIntoDescendant,
}

/// 사이드바 드래그 시 드롭 위치. egui-arbor/egui_ltreeview가 이 3종을 그대로 지원.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropPosition {
    Before,
    After,
    /// 대상 노드의 자식으로 편입 (하위 트리 이동은 해당 없음, 이동하는 노드 자체가 통째로 들어감)
    Inside,
}

/// depth 순차값 기반으로 트리를 복원한다.
///
/// 알고리즘: 행 순서대로 읽으며 "각 depth에서 마지막으로 추가된 노드의 인덱스 경로"를 스택으로 유지.
/// depth가 이전보다 크면 직전 행의 자식으로, 같거나 작으면 그 depth까지 스택을 pop 후 해당 부모에 편입.
/// → 행 순서가 트리 구조를 결정하므로 rows는 반드시 depth-first 순서여야 한다.
pub fn build_tree(rows: &[BookmarkRow]) -> Vec<BookmarkNode> {
    let mut roots: Vec<BookmarkNode> = Vec::new();
    let mut path: Vec<usize> = Vec::new();

    for row in rows {
        let depth = row.depth as usize;
        path.truncate(depth);

        let new_node = BookmarkNode::new(row.title.clone(), row.page);
        let siblings = children_at_path_mut(&mut roots, &path);
        siblings.push(new_node);
        let new_index = siblings.len() - 1;
        path.push(new_index);
    }

    roots
}

/// 트리를 depth-first 순서로 평탄화한다 (export용). build_tree의 역연산.
pub fn flatten_tree(nodes: &[BookmarkNode], filename: &str) -> Vec<BookmarkRow> {
    let mut rows = Vec::new();
    flatten_rec(nodes, filename, 0, &mut rows);
    rows
}

fn flatten_rec(nodes: &[BookmarkNode], filename: &str, depth: u32, out: &mut Vec<BookmarkRow>) {
    for node in nodes {
        out.push(BookmarkRow {
            filename: filename.to_string(),
            depth,
            title: node.title.clone(),
            page: node.page,
        });
        flatten_rec(&node.children, filename, depth + 1, out);
    }
}

fn children_at_path_mut<'a>(
    roots: &'a mut Vec<BookmarkNode>,
    path: &[usize],
) -> &'a mut Vec<BookmarkNode> {
    let mut current = roots;
    for &idx in path {
        current = &mut current[idx].children;
    }
    current
}

/// parent_id 노드의 자식으로 새 노드를 추가한다. parent_id가 None이면 최상위(루트)에 추가.
/// parent_id를 줬는데 트리에서 못 찾으면 최상위에 추가한다 — 항목이 조용히 사라지는 것보다 안전.
pub fn insert_node(nodes: &mut Vec<BookmarkNode>, parent_id: Option<Uuid>, node: BookmarkNode) {
    match parent_id {
        None => nodes.push(node),
        Some(pid) => {
            if let Some(unplaced) = insert_into(nodes, pid, node) {
                nodes.push(unplaced);
            }
        }
    }
}

/// 삽입에 성공하면 None, parent_id를 못 찾았으면 소유권을 그대로 돌려준다(Some(node)).
fn insert_into(nodes: &mut [BookmarkNode], parent_id: Uuid, node: BookmarkNode) -> Option<BookmarkNode> {
    let mut node = node;
    for n in nodes.iter_mut() {
        if n.id == parent_id {
            n.children.push(node);
            return None;
        }
        match insert_into(&mut n.children, parent_id, node) {
            None => return None,
            Some(returned) => node = returned,
        }
    }
    Some(node)
}

/// moving_id 노드를(하위 트리 포함 통째로) 찾아 제거하고 반환한다.
pub fn remove_node(nodes: &mut Vec<BookmarkNode>, id: Uuid) -> Option<BookmarkNode> {
    if let Some(pos) = nodes.iter().position(|n| n.id == id) {
        return Some(nodes.remove(pos));
    }
    for node in nodes.iter_mut() {
        if let Some(found) = remove_node(&mut node.children, id) {
            return Some(found);
        }
    }
    None
}

fn contains_id(node: &BookmarkNode, id: Uuid) -> bool {
    node.id == id || node.children.iter().any(|c| contains_id(c, id))
}

fn insert_relative(
    nodes: &mut Vec<BookmarkNode>,
    target_id: Uuid,
    position: DropPosition,
    node: BookmarkNode,
) -> bool {
    if let Some(pos) = nodes.iter().position(|n| n.id == target_id) {
        match position {
            DropPosition::Before => nodes.insert(pos, node),
            DropPosition::After => nodes.insert(pos + 1, node),
            DropPosition::Inside => nodes[pos].children.push(node),
        }
        return true;
    }
    for n in nodes.iter_mut() {
        if insert_relative(&mut n.children, target_id, position, node.clone()) {
            return true;
        }
    }
    false
}

/// 사이드바 드래그 재구성: moving_id 노드(하위 트리 포함)를 target_id 기준
/// Before/After/Inside 위치로 이동시킨다.
pub fn move_node(
    roots: &mut Vec<BookmarkNode>,
    moving_id: Uuid,
    target_id: Uuid,
    position: DropPosition,
) -> Result<(), TreeError> {
    if moving_id == target_id {
        return Err(TreeError::InvalidMoveIntoDescendant);
    }

    // 자기 자신의 하위 트리 안으로는 이동 불가 (사이클 방지)
    let moving_ref = find_node(roots, moving_id).ok_or(TreeError::TargetNotFound(moving_id))?;
    if contains_id(moving_ref, target_id) {
        return Err(TreeError::InvalidMoveIntoDescendant);
    }

    let removed = remove_node(roots, moving_id).ok_or(TreeError::TargetNotFound(moving_id))?;

    if !insert_relative(roots, target_id, position, removed.clone()) {
        // 삽입 실패 시 원상복구는 하지 않음(호출측에서 트랜잭션/undo 스택으로 관리 권장)
        return Err(TreeError::TargetNotFound(target_id));
    }
    Ok(())
}

fn find_node(nodes: &[BookmarkNode], id: Uuid) -> Option<&BookmarkNode> {
    for n in nodes {
        if n.id == id {
            return Some(n);
        }
        if let Some(found) = find_node(&n.children, id) {
            return Some(found);
        }
    }
    None
}

/// id 노드의 부모 id를 찾는다. 최상위 노드거나 트리에 없으면 None.
/// 사이드바 화살표 키(리프 노드에서 좌/우로 부모 레벨을 접고 펴기)에 쓰인다.
pub fn parent_of(nodes: &[BookmarkNode], id: Uuid) -> Option<Uuid> {
    for n in nodes {
        if n.children.iter().any(|c| c.id == id) {
            return Some(n.id);
        }
        if let Some(p) = parent_of(&n.children, id) {
            return Some(p);
        }
    }
    None
}

/// id 노드를 지운 뒤 대신 선택할 만한 노드를 고른다: 다음 형제 → 이전 형제 → 부모 순.
/// 셋 다 없으면(형제도 부모도 없는 유일한 최상위 노드) None — 삭제 후 선택 해제.
/// 삭제 "전에" 호출해야 한다(삭제 후에는 형제/부모 관계를 더 이상 알 수 없음).
pub fn sibling_or_parent_after_removal(nodes: &[BookmarkNode], id: Uuid) -> Option<Uuid> {
    fn find_siblings(nodes: &[BookmarkNode], id: Uuid) -> Option<(&[BookmarkNode], usize)> {
        if let Some(pos) = nodes.iter().position(|n| n.id == id) {
            return Some((nodes, pos));
        }
        for n in nodes {
            if let Some(found) = find_siblings(&n.children, id) {
                return Some(found);
            }
        }
        None
    }

    let (siblings, pos) = find_siblings(nodes, id)?;
    if pos + 1 < siblings.len() {
        Some(siblings[pos + 1].id)
    } else if pos > 0 {
        Some(siblings[pos - 1].id)
    } else {
        parent_of(nodes, id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(filename: &str, depth: u32, title: &str, page: u32) -> BookmarkRow {
        BookmarkRow {
            filename: filename.to_string(),
            depth,
            title: title.to_string(),
            page,
        }
    }

    #[test]
    fn build_tree_simple_hierarchy() {
        let rows = vec![
            row("a.pdf", 0, "1장", 1),
            row("a.pdf", 1, "1.1절", 2),
            row("a.pdf", 1, "1.2절", 5),
            row("a.pdf", 0, "2장", 10),
            row("a.pdf", 1, "2.1절", 11),
            row("a.pdf", 2, "2.1.1항", 12),
        ];
        let tree = build_tree(&rows);

        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].title, "1장");
        assert_eq!(tree[0].children.len(), 2);
        assert_eq!(tree[0].children[0].title, "1.1절");
        assert_eq!(tree[0].children[1].title, "1.2절");

        assert_eq!(tree[1].title, "2장");
        assert_eq!(tree[1].children.len(), 1);
        assert_eq!(tree[1].children[0].title, "2.1절");
        assert_eq!(tree[1].children[0].children.len(), 1);
        assert_eq!(tree[1].children[0].children[0].title, "2.1.1항");
    }

    #[test]
    fn flatten_is_inverse_of_build() {
        let rows = vec![
            row("doc.pdf", 0, "루트A", 1),
            row("doc.pdf", 1, "자식1", 2),
            row("doc.pdf", 2, "손자1", 3),
            row("doc.pdf", 1, "자식2", 4),
            row("doc.pdf", 0, "루트B", 9),
        ];
        let tree = build_tree(&rows);
        let flattened = flatten_tree(&tree, "doc.pdf");
        assert_eq!(rows, flattened);
    }

    #[test]
    fn move_node_preserves_subtree() {
        let rows = vec![
            row("a.pdf", 0, "루트1", 1),
            row("a.pdf", 1, "자식1-1", 2),
            row("a.pdf", 1, "자식1-2", 3),
            row("a.pdf", 0, "루트2", 4),
        ];
        let mut tree = build_tree(&rows);

        let root1_id = tree[0].id;
        let root2_id = tree[1].id;

        // 루트1(하위 자식 2개 포함)을 루트2 "안"으로 이동
        move_node(&mut tree, root1_id, root2_id, DropPosition::Inside).unwrap();

        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].id, root2_id);
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].id, root1_id);
        // 하위 트리(자식1-1, 자식1-2)가 통째로 따라왔는지 확인
        assert_eq!(tree[0].children[0].children.len(), 2);
        assert_eq!(tree[0].children[0].children[0].title, "자식1-1");
        assert_eq!(tree[0].children[0].children[1].title, "자식1-2");
    }

    #[test]
    fn move_node_into_own_descendant_rejected() {
        let rows = vec![row("a.pdf", 0, "부모", 1), row("a.pdf", 1, "자식", 2)];
        let mut tree = build_tree(&rows);
        let parent_id = tree[0].id;
        let child_id = tree[0].children[0].id;

        let result = move_node(&mut tree, parent_id, child_id, DropPosition::Inside);
        assert!(matches!(result, Err(TreeError::InvalidMoveIntoDescendant)));
    }

    #[test]
    fn insert_node_with_no_parent_goes_to_root() {
        let mut tree = build_tree(&[row("a.pdf", 0, "루트1", 1)]);
        insert_node(&mut tree, None, BookmarkNode::new("새 항목", 2));
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[1].title, "새 항목");
    }

    #[test]
    fn insert_node_with_parent_becomes_child() {
        let mut tree = build_tree(&[row("a.pdf", 0, "루트1", 1)]);
        let parent_id = tree[0].id;
        insert_node(&mut tree, Some(parent_id), BookmarkNode::new("자식", 2));
        assert_eq!(tree[0].children.len(), 1);
        assert_eq!(tree[0].children[0].title, "자식");
    }

    #[test]
    fn insert_node_with_unknown_parent_falls_back_to_root() {
        let mut tree = build_tree(&[row("a.pdf", 0, "루트1", 1)]);
        insert_node(&mut tree, Some(Uuid::new_v4()), BookmarkNode::new("고아", 2));
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[1].title, "고아");
    }

    #[test]
    fn remove_node_is_public_and_removes_subtree() {
        let rows = vec![row("a.pdf", 0, "부모", 1), row("a.pdf", 1, "자식", 2)];
        let mut tree = build_tree(&rows);
        let parent_id = tree[0].id;
        let removed = remove_node(&mut tree, parent_id);
        assert!(removed.is_some());
        assert!(tree.is_empty());
    }

    #[test]
    fn parent_of_finds_immediate_parent() {
        let rows = vec![row("a.pdf", 0, "부모", 1), row("a.pdf", 1, "자식", 2)];
        let tree = build_tree(&rows);
        let parent_id = tree[0].id;
        let child_id = tree[0].children[0].id;
        assert_eq!(parent_of(&tree, child_id), Some(parent_id));
        assert_eq!(parent_of(&tree, parent_id), None);
    }

    #[test]
    fn sibling_or_parent_after_removal_prefers_next_sibling() {
        let rows = vec![
            row("a.pdf", 0, "루트", 1),
            row("a.pdf", 1, "자식1", 2),
            row("a.pdf", 1, "자식2", 3),
            row("a.pdf", 1, "자식3", 4),
        ];
        let tree = build_tree(&rows);
        let child1 = tree[0].children[0].id;
        let child2 = tree[0].children[1].id;
        assert_eq!(sibling_or_parent_after_removal(&tree, child1), Some(child2));
    }

    #[test]
    fn sibling_or_parent_after_removal_falls_back_to_previous_sibling() {
        let rows = vec![
            row("a.pdf", 0, "루트", 1),
            row("a.pdf", 1, "자식1", 2),
            row("a.pdf", 1, "자식2", 3),
        ];
        let tree = build_tree(&rows);
        let child1 = tree[0].children[0].id;
        let child2 = tree[0].children[1].id;
        // child2가 마지막(다음 형제 없음) -> 이전 형제(child1)로
        assert_eq!(sibling_or_parent_after_removal(&tree, child2), Some(child1));
    }

    #[test]
    fn sibling_or_parent_after_removal_falls_back_to_parent() {
        let rows = vec![row("a.pdf", 0, "루트", 1), row("a.pdf", 1, "외동자식", 2)];
        let tree = build_tree(&rows);
        let root_id = tree[0].id;
        let only_child = tree[0].children[0].id;
        // 형제가 없는 유일한 자식 -> 부모로
        assert_eq!(sibling_or_parent_after_removal(&tree, only_child), Some(root_id));
    }

    #[test]
    fn sibling_or_parent_after_removal_none_when_sole_root() {
        let tree = build_tree(&[row("a.pdf", 0, "유일한 루트", 1)]);
        let id = tree[0].id;
        assert_eq!(sibling_or_parent_after_removal(&tree, id), None);
    }
}
