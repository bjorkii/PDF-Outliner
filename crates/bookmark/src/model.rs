use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// DB/저장소에 들어가는 flat 레코드. 계층은 parent_id로 표현.
/// (사이드바 드래그 재구성 시 이 구조를 직접 갱신)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Bookmark {
    pub id: Uuid,
    pub parent_id: Option<Uuid>,
    pub title: String,
    /// 1-based 페이지 번호
    pub page: u32,
    /// 같은 부모 밑에서의 정렬 순서
    pub order: i32,
    pub zoom: Option<f32>,
    pub scroll_y: Option<f32>,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
}

impl Bookmark {
    pub fn new(title: impl Into<String>, page: u32) -> Self {
        Self {
            id: Uuid::new_v4(),
            parent_id: None,
            title: title.into(),
            page,
            order: 0,
            zoom: None,
            scroll_y: None,
            tags: Vec::new(),
            created_at: Utc::now(),
        }
    }
}

/// CSV/Excel import·export용 flat row.
/// 스키마: 파일명 / 계층 / 북마크명 / 페이지번호
/// 행 순서 자체가 트리 구조를 결정하므로(depth-first 필수) 순서 보존이 중요하다.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BookmarkRow {
    pub filename: String,
    pub depth: u32,
    pub title: String,
    pub page: u32,
}

/// UI(사이드바 트리)/import 파싱 결과용 재귀 트리 노드.
/// Serialize/Deserialize는 크래시 복구용 자동저장 스냅샷(`crates/ui/src/autosave.rs`)에
/// 그대로 JSON 직렬화하기 위해 필요하다.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BookmarkNode {
    pub id: Uuid,
    pub title: String,
    pub page: u32,
    pub children: Vec<BookmarkNode>,
}

impl BookmarkNode {
    pub fn new(title: impl Into<String>, page: u32) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            page,
            children: Vec::new(),
        }
    }
}
