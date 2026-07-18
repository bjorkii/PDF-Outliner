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
            // 1-based 일련번호 — import 시 이 값으로 정렬해 트리를 복원하므로(model.rs
            // 참고) depth-first 나열 순서 그대로 매겨져야 한다.
            order: out.len() as u32 + 1,
            filename: filename.to_string(),
            depth,
            title: node.title.clone(),
            page: node.page,
        });
        flatten_rec(&node.children, filename, depth + 1, out);
    }
}

/// import된 행들을 트리로 만들기 전 정리(2026-07-19 요청): (1) '파일명' 컬럼이 현재 열린
/// 파일명과 일치하는 행만 남기고, (2) 남은 행을 '순서' 컬럼 기준으로 정렬한다 — 사용자가
/// Excel에서 행을 뒤섞거나 다른 문서의 행이 섞인 파일을 가져와도 안전하다.
/// 반환: (정리된 행들, 파일명 불일치로 걸러진 행 수).
pub fn prepare_imported_rows(
    mut rows: Vec<BookmarkRow>,
    current_filename: &str,
) -> (Vec<BookmarkRow>, usize) {
    let before = rows.len();
    rows.retain(|r| r.filename == current_filename);
    let skipped = before - rows.len();
    // 같은 순서값이 중복돼 있어도(손으로 편집하다 실수) 원래 행 순서를 보존하도록 stable sort.
    rows.sort_by_key(|r| r.order);
    (rows, skipped)
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

/// `insert_node`(항상 형제 목록 맨 끝)를 대체하는, 페이지 번호 순서를 지키는 삽입.
/// 사이드바 "+"/Cmd+B(`crates/ui/src/app.rs`의 `add_bookmark_under_selection`)가 쓴다.
///
/// - `parent_id`가 `Some(pid)`: pid 노드의 자식 목록 안에서 `node.page` 기준 정렬 위치에
///   삽입(같은 페이지의 기존 항목이 있으면 그 뒤). pid를 못 찾으면 최상위에서 같은 규칙으로
///   삽입한다(기존 `insert_node`와 동일한 폴백 관례).
/// - `parent_id`가 `None`: 특정 부모를 지정하지 않은 경우다. 트리 전체를 depth-first로 훑어
///   "페이지가 `node.page` 이하인 마지막 노드"(anchor)를 찾고, **anchor의 자식이 아니라
///   anchor와 같은 레벨(형제)**로, anchor 바로 뒤에 끼워 넣는다 — "선택된 북마크가 없으면
///   페이지 순서상 직전 북마크와 같은 레벨로 생성" 요구사항이 이 한 번의 탐색으로 충족된다.
///   anchor가 없으면(모든 기존 북마크보다 페이지가 앞섬) 최상위 목록에서 페이지 순서 위치에
///   삽입한다.
pub fn insert_node_by_page(nodes: &mut Vec<BookmarkNode>, parent_id: Option<Uuid>, node: BookmarkNode) {
    match parent_id {
        Some(pid) => {
            if let Some(unplaced) = insert_into_children_ordered(nodes, pid, node) {
                insert_ordered(nodes, unplaced);
            }
        }
        None => {
            let mut paths = Vec::new();
            let mut prefix = Vec::new();
            collect_paths(nodes, &mut prefix, &mut paths);
            let anchor_path = paths
                .into_iter()
                .rev()
                .find(|(_, page)| *page <= node.page)
                .map(|(path, _)| path);

            match anchor_path {
                Some(path) => {
                    let idx = *path.last().expect("collect_paths never returns an empty path");
                    let siblings = children_at_path_mut(nodes, &path[..path.len() - 1]);
                    siblings.insert(idx + 1, node);
                }
                None => insert_ordered(nodes, node),
            }
        }
    }
}

/// 같은 형제 목록 안에서 페이지 순서 위치에 삽입 — 같은 페이지가 이미 있으면 그 뒤(그
/// 페이지 그룹 안에서는 나중에 추가한 게 끝에 붙는 관례. 페이지 내 세부 위치 개념 자체가
/// 없으므로 — pdfium outline에도 그런 좌표가 없다 — 이 순서가 곧 CSV/Excel export 순서와
/// 사용자가 드래그로 수동 재배열한 결과의 유일한 근거가 된다).
fn insert_ordered(siblings: &mut Vec<BookmarkNode>, node: BookmarkNode) {
    let pos = siblings
        .iter()
        .position(|s| s.page > node.page)
        .unwrap_or(siblings.len());
    siblings.insert(pos, node);
}

/// pid 노드를 찾으면 그 자식 목록에 페이지 순서로 삽입하고 None, 못 찾으면 소유권을
/// 돌려준다(Some(node)) — `insert_into`와 동일한 재귀/폴백 패턴, 정렬 삽입만 다르다.
fn insert_into_children_ordered(
    nodes: &mut [BookmarkNode],
    pid: Uuid,
    node: BookmarkNode,
) -> Option<BookmarkNode> {
    let mut node = node;
    for n in nodes.iter_mut() {
        if n.id == pid {
            insert_ordered(&mut n.children, node);
            return None;
        }
        match insert_into_children_ordered(&mut n.children, pid, node) {
            None => return None,
            Some(returned) => node = returned,
        }
    }
    Some(node)
}

/// (path, page) 목록을 depth-first 순서로 수집한다 — path의 각 원소는 그 깊이에서의
/// 형제 목록 내 인덱스(마지막 원소가 자기 자신의 인덱스). `children_at_path_mut`(build_tree가
/// 쓰는 것과 동일)로 `path[..len-1]`을 넘기면 그 노드의 부모 형제 목록을 다시 얻을 수 있다.
fn collect_paths(nodes: &[BookmarkNode], prefix: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, u32)>) {
    for (i, n) in nodes.iter().enumerate() {
        prefix.push(i);
        out.push((prefix.clone(), n.page));
        collect_paths(&n.children, prefix, out);
        prefix.pop();
    }
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
/// 뷰어에 표시 중인 페이지에 대응하는 "활성" 북마크를 찾는다(사이드바 볼드 표시용,
/// `crates/ui/src/sidebar.rs`). 정확히 그 페이지의 북마크가 없으면, 페이지 번호가 그 이하인
/// 것 중 트리 depth-first 순서상 가장 마지막(= 문서 흐름상 가장 가까운 이전) 노드를 반환한다
/// — `insert_node_by_page`의 anchor 탐색과 같은 규칙(사용자가 "가장 가까운 이전 북마크를
/// 볼드"로 확정, 2026-07-14). 모든 북마크보다 현재 페이지가 앞서면 None.
pub fn active_bookmark_for_page(nodes: &[BookmarkNode], page: u32) -> Option<Uuid> {
    let mut flat = Vec::new();
    collect_ids_dfs(nodes, &mut flat);
    flat.into_iter().rev().find(|(_, p)| *p <= page).map(|(id, _)| id)
}

fn collect_ids_dfs(nodes: &[BookmarkNode], out: &mut Vec<(Uuid, u32)>) {
    for n in nodes {
        out.push((n.id, n.page));
        collect_ids_dfs(&n.children, out);
    }
}

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
            order: 0,
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
        // flatten은 depth-first 순서대로 1-based 일련번호(order)를 새로 매긴다.
        let expected: Vec<BookmarkRow> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| BookmarkRow { order: i as u32 + 1, ..r.clone() })
            .collect();
        assert_eq!(expected, flattened);
    }

    #[test]
    fn prepare_imported_rows_sorts_by_order_and_filters_filename() {
        // 행이 뒤섞여 있고 다른 문서의 행이 섞인 파일 — '순서'로 정렬되고 파일명 불일치는
        // 걸러져야 한다(2026-07-19 요구사항 그대로).
        let mk = |order: u32, filename: &str, title: &str| BookmarkRow {
            order,
            filename: filename.to_string(),
            depth: 0,
            title: title.to_string(),
            page: 1,
        };
        let rows = vec![
            mk(3, "test.pdf", "셋째"),
            mk(1, "test.pdf", "첫째"),
            mk(2, "other.pdf", "남의 것"),
            mk(2, "test.pdf", "둘째"),
        ];
        let (kept, skipped) = prepare_imported_rows(rows, "test.pdf");
        assert_eq!(skipped, 1);
        assert_eq!(
            kept.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            vec!["첫째", "둘째", "셋째"]
        );
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

    /// 사용자가 준 예시 그대로: A(34쪽)/B(37쪽)가 최상위에 있을 때 선택 없이(parent_id=None)
    /// 35쪽 북마크를 추가하면 A와 B 사이, 최상위(형제) 레벨에 들어가야 한다.
    #[test]
    fn insert_by_page_no_selection_lands_between_siblings_at_same_level() {
        let mut tree = vec![
            BookmarkNode::new("A", 34),
            BookmarkNode::new("B", 37),
        ];
        insert_node_by_page(&mut tree, None, BookmarkNode::new("새 북마크", 35));

        assert_eq!(tree.len(), 3);
        assert_eq!(tree[0].title, "A");
        assert_eq!(tree[1].title, "새 북마크");
        assert_eq!(tree[2].title, "B");
    }

    /// anchor가 중첩된 자식일 때도, anchor의 자식이 아니라 anchor와 같은 레벨(형제)에 들어가야
    /// 한다 — "선택 없으면 직전 북마크와 같은 레벨" 요구사항.
    #[test]
    fn insert_by_page_no_selection_matches_anchor_level_not_its_children() {
        let mut tree = vec![BookmarkNode::new("1장", 1)];
        let chapter_id = tree[0].id;
        insert_node_by_page(&mut tree, Some(chapter_id), BookmarkNode::new("1.1절", 2));

        // 지금 트리: 1장(1쪽) -> 1.1절(2쪽). 선택 없이 3쪽을 추가하면 anchor는 1.1절(가장 최근
        // 페이지가 3 이하인 노드)이고, 1.1절의 자식이 아니라 1.1절과 같은 레벨 —
        // 즉 1장의 children 목록에 형제로 들어가야 한다.
        insert_node_by_page(&mut tree, None, BookmarkNode::new("1.2절", 3));

        assert_eq!(tree.len(), 1, "최상위에 새로 생기면 안 됨 — 1.1절과 같은 레벨(1장의 자식)이어야 함");
        assert_eq!(tree[0].children.len(), 2);
        assert_eq!(tree[0].children[0].title, "1.1절");
        assert_eq!(tree[0].children[1].title, "1.2절");
    }

    /// anchor가 없을 때(모든 기존 북마크보다 페이지가 앞섬) 최상위 맨 앞에 들어간다.
    #[test]
    fn insert_by_page_no_selection_no_anchor_goes_to_front() {
        let mut tree = vec![BookmarkNode::new("B", 37)];
        insert_node_by_page(&mut tree, None, BookmarkNode::new("A", 10));

        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].title, "A");
        assert_eq!(tree[1].title, "B");
    }

    /// 선택된 노드가 있으면 그 자식으로 들어가되(기존 관례 유지), 그 자식들 사이에서도
    /// 페이지 순서를 지킨다.
    #[test]
    fn insert_by_page_with_selection_orders_within_that_parents_children() {
        let mut tree = vec![BookmarkNode::new("장", 1)];
        let parent_id = tree[0].id;
        insert_node_by_page(&mut tree, Some(parent_id), BookmarkNode::new("절2", 5));
        insert_node_by_page(&mut tree, Some(parent_id), BookmarkNode::new("절1", 3));

        assert_eq!(tree[0].children.len(), 2);
        assert_eq!(tree[0].children[0].title, "절1");
        assert_eq!(tree[0].children[1].title, "절2");
    }

    /// 같은 페이지에 이미 북마크가 있으면, 새로 추가되는 게 그 뒤에 붙는다(페이지 내 세부
    /// 위치 개념이 없으므로 이게 유일한 순서 근거 — model.rs 주석 참고).
    #[test]
    fn insert_by_page_same_page_appends_after_existing() {
        let mut tree = vec![BookmarkNode::new("먼저", 5)];
        insert_node_by_page(&mut tree, None, BookmarkNode::new("나중", 5));

        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].title, "먼저");
        assert_eq!(tree[1].title, "나중");
    }

    /// parent_id를 지정했는데 트리에서 못 찾으면(예: 그 사이 삭제됨) 조용히 사라지지 않고
    /// 최상위에서 페이지 순서로 폴백 — 기존 insert_node의 폴백 관례와 동일.
    #[test]
    fn insert_by_page_missing_parent_falls_back_to_top_level_ordered() {
        let mut tree = vec![BookmarkNode::new("A", 10), BookmarkNode::new("C", 30)];
        insert_node_by_page(&mut tree, Some(Uuid::new_v4()), BookmarkNode::new("B", 20));

        assert_eq!(tree.len(), 3);
        assert_eq!(tree[1].title, "B");
    }

    #[test]
    fn active_bookmark_exact_page_match() {
        let tree = vec![BookmarkNode::new("A", 10), BookmarkNode::new("B", 20)];
        assert_eq!(active_bookmark_for_page(&tree, 20), Some(tree[1].id));
    }

    /// 북마크 사이 페이지는 가장 가까운 이전 북마크가 활성 상태여야 한다.
    #[test]
    fn active_bookmark_between_pages_picks_nearest_preceding() {
        let tree = vec![BookmarkNode::new("A", 10), BookmarkNode::new("B", 20)];
        assert_eq!(active_bookmark_for_page(&tree, 15), Some(tree[0].id));
    }

    /// 중첩된 자식이 더 뒤 페이지를 가리키면 그쪽이 anchor가 된다(depth-first 순서 반영).
    #[test]
    fn active_bookmark_prefers_nested_child_when_later_in_dfs_order() {
        let mut tree = vec![BookmarkNode::new("1장", 1)];
        let chapter_id = tree[0].id;
        insert_node_by_page(&mut tree, Some(chapter_id), BookmarkNode::new("1.1절", 2));
        let child_id = tree[0].children[0].id;

        assert_eq!(active_bookmark_for_page(&tree, 3), Some(child_id));
    }

    /// 모든 북마크보다 앞선 페이지에서는 활성 북마크가 없다.
    #[test]
    fn active_bookmark_none_before_first_bookmark() {
        let tree = vec![BookmarkNode::new("A", 10)];
        assert_eq!(active_bookmark_for_page(&tree, 5), None);
    }
}
