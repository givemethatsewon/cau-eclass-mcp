//! Pure catalog logic for Canvas files and LearningX weekly materials.

use std::collections::HashMap;

use regex::Regex;
use serde::Serialize;
use serde_json::{Map, Value};
use unicode_normalization::UnicodeNormalization;

use crate::error::EclassError;

/// A unified Commons or Canvas material entry.
#[derive(Debug, Clone, Serialize)]
pub struct Material {
    pub resource_id: String,
    pub course_id: u32,
    pub title: String,
    pub week: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub weeks: Option<Vec<u32>>,
    pub source: String,
    pub content_type: String,
    pub available: bool,
    pub download_supported: bool,
    pub text_supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub board_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachment_id: Option<u32>,
    #[serde(skip)]
    pub download_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

/// Parsed resource identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceId {
    pub source: String,
    pub course_id: u32,
    pub parts: Vec<u32>,
}

impl ResourceId {
    /// First provider-specific ID (Commons item, Canvas file, board attachment).
    #[must_use]
    pub fn source_id(&self) -> Option<u32> {
        self.parts.first().copied()
    }
}

/// A compact weekly module.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleView {
    pub module_id: Value,
    pub title: String,
    pub week: Option<u32>,
    pub items: Vec<Value>,
}

/// Parse a term name such as `2026년 2학기`, `2021-1학기`, or `2026 Fall`
/// into `(year, semester)`.
#[must_use]
pub fn term_key(name: &str) -> Option<(i32, u32)> {
    let korean = Regex::new(r"(\d{4})\D{0,5}(\d{1,2})\s*학기").ok()?;
    if let Some(captures) = korean.captures(name) {
        let year = captures.get(1)?.as_str().parse().ok()?;
        let semester = captures.get(2)?.as_str().parse().ok()?;
        return (1..=4).contains(&semester).then_some((year, semester));
    }
    let seasonal = Regex::new(r"(?i)(spring|summer|fall|autumn|winter)\s*(\d{4})|(\d{4})\s*(spring|summer|fall|autumn|winter)").ok()?;
    let captures = seasonal.captures(name)?;
    let (season, year) = if let (Some(season), Some(year)) = (captures.get(1), captures.get(2)) {
        (season.as_str(), year.as_str())
    } else {
        (captures.get(4)?.as_str(), captures.get(3)?.as_str())
    };
    let semester = match season.to_ascii_lowercase().as_str() {
        "spring" | "summer" => 1,
        _ => 2,
    };
    Some((year.parse().ok()?, semester))
}

/// The academic term that contains a calendar month.
///
/// CAU runs 1학기 March–August (including the summer session) and 2학기
/// September–February, so January and February belong to the previous year.
#[must_use]
pub fn term_key_for_month(year: i32, month: u32) -> (i32, u32) {
    match month {
        1 | 2 => (year - 1, 2),
        3..=8 => (year, 1),
        _ => (year, 2),
    }
}

/// Match a course title/code and a term ID or name fragment.
#[must_use]
pub fn course_matches(course: &Value, query: Option<&str>, term: Option<&str>) -> bool {
    let search = normalized(query);
    if !search.is_empty() {
        let matches = ["name", "course_code"].iter().any(|key| {
            course
                .get(*key)
                .and_then(Value::as_str)
                .map(|value| normalized(Some(value)))
                .is_some_and(|value| value.contains(&search))
        });
        if !matches {
            return false;
        }
    }
    let term_search = normalized(term);
    if term_search.is_empty() {
        return true;
    }
    let course_term = course.get("term");
    let (term_id, term_name) = if let Some(term) = course_term.and_then(Value::as_object) {
        (
            term.get("id").and_then(positive_id),
            term.get("name").and_then(Value::as_str),
        )
    } else {
        (
            course.get("enrollment_term_id").and_then(positive_id),
            course_term.and_then(Value::as_str),
        )
    };
    term_id.is_some_and(|id| term_search == id.to_string())
        || term_name.is_some_and(|name| normalized(Some(name)).contains(&term_search))
}

/// Parse `commons:<course>:<item>`, `canvas:<course>:<file>`, or
/// `board:<course>:<board>:<post>:<attachment>`.
pub fn parse_resource_id(value: &str) -> Result<ResourceId, EclassError> {
    let pattern = Regex::new(r"^(commons|canvas|board):([1-9][0-9]*)(?::([1-9][0-9]*))*$")
        .map_err(|_| EclassError::InvalidPath)?;
    if !pattern.is_match(value) {
        return Err(EclassError::MaterialNotFound);
    }
    let mut chunks = value.split(':');
    let source = chunks.next().unwrap_or_default().to_string();
    let course_id = chunks
        .next()
        .and_then(|value| value.parse::<u32>().ok())
        .ok_or(EclassError::MaterialNotFound)?;
    let parts = chunks
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| EclassError::MaterialNotFound)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected = if source == "board" { 3 } else { 1 };
    if parts.len() != expected || parts.contains(&0) {
        return Err(EclassError::MaterialNotFound);
    }
    Ok(ResourceId {
        source,
        course_id,
        parts,
    })
}

/// Extract a week number from titles such as `제 11주차 강의자료`.
#[must_use]
pub fn week_from_title(value: &str) -> Option<u32> {
    let pattern = Regex::new(r"([0-9]{1,2})\s*(?:주차|주|강)").ok()?;
    pattern
        .captures(value)
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse().ok())
}

/// Flatten Canvas files and LearningX module items into a stable catalog.
#[must_use]
pub fn material_catalog(course_id: u32, modules: &[Value], files: &[Value]) -> Vec<Material> {
    let by_file_id: HashMap<u32, &Value> = files
        .iter()
        .filter_map(|file| positive_id(file.get("id")?).map(|id| (id, file)))
        .collect();
    let mut resources = Vec::new();
    let mut index = HashMap::new();

    for module in modules {
        for item in module_items(module) {
            let resource = commons_resource(course_id, module, item).or_else(|| {
                is_canvas_file(item).then(|| {
                    canvas_resource(
                        course_id,
                        module,
                        item,
                        item.get("content_id")
                            .and_then(positive_id)
                            .and_then(|id| by_file_id.get(&id).copied()),
                    )
                })?
            });
            if let Some(resource) = resource {
                add_resource(&mut resources, &mut index, resource);
            }
        }
    }
    for file in files {
        if let Some(resource) = canvas_resource(course_id, &Value::Null, &Value::Null, Some(file)) {
            add_resource(&mut resources, &mut index, resource);
        }
    }
    resources
}

/// Apply week, title and type filters.
#[must_use]
pub fn filter_materials(
    items: &[Material],
    week: Option<u32>,
    query: Option<&str>,
    content_type: Option<&str>,
) -> Vec<Material> {
    let search = normalized(query);
    let kind = content_type.map(|value| content_type_name(Some(value), None));
    items
        .iter()
        .filter(|item| {
            let week_ok = week.is_none_or(|week| {
                item.week == Some(week)
                    || item
                        .weeks
                        .as_ref()
                        .is_some_and(|weeks| weeks.contains(&week))
            });
            let query_ok = search.is_empty() || normalized(Some(&item.title)).contains(&search);
            let type_ok = kind
                .as_ref()
                .is_none_or(|kind| content_type_name(Some(&item.content_type), None) == *kind);
            week_ok && query_ok && type_ok
        })
        .cloned()
        .collect()
}

/// Build compact weekly modules with optional filters.
#[must_use]
pub fn compact_modules(
    course_id: u32,
    modules: &[Value],
    week: Option<u32>,
    query: Option<&str>,
    content_type: Option<&str>,
) -> Vec<ModuleView> {
    let filtered = week.is_some() || !normalized(query).is_empty() || content_type.is_some();
    let mut result = Vec::new();
    for module in modules {
        let module_week = week_of(module, None);
        let mut items = Vec::new();
        for item in module_items(module) {
            let resource = commons_resource(course_id, module, item)
                .or_else(|| {
                    is_canvas_file(item).then(|| canvas_resource(course_id, module, item, None))?
                })
                .or_else(|| {
                    let data = data(item);
                    let reason = unavailable_reason(&[module.as_object(), item.as_object(), data]);
                    Some(Material {
                        resource_id: String::new(),
                        course_id,
                        title: item
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or("Untitled item")
                            .to_string(),
                        week: module_week,
                        weeks: None,
                        source: "learningx".to_string(),
                        content_type: item
                            .get("content_type")
                            .or_else(|| item.get("type"))
                            .and_then(Value::as_str)
                            .unwrap_or("unknown")
                            .to_string(),
                        available: reason.is_none(),
                        download_supported: false,
                        text_supported: false,
                        created_at: None,
                        updated_at: None,
                        view_url: None,
                        post_title: None,
                        board_id: None,
                        post_id: None,
                        attachment_id: None,
                        download_url: None,
                        unavailable_reason: reason,
                    })
                });
            let Some(resource) = resource else {
                continue;
            };
            if week.is_some() && resource.week != week {
                continue;
            }
            if let Some(kind) = content_type
                && content_type_name(Some(&resource.content_type), None)
                    != content_type_name(Some(kind), None)
            {
                continue;
            }
            if !normalized(query).is_empty()
                && !normalized(Some(&resource.title)).contains(&normalized(query))
            {
                continue;
            }
            items.push(serde_json::to_value(resource).unwrap_or(Value::Null));
        }
        if !filtered || !items.is_empty() {
            result.push(ModuleView {
                module_id: module
                    .get("module_id")
                    .or_else(|| module.get("id"))
                    .cloned()
                    .unwrap_or(Value::Null),
                title: module
                    .get("title")
                    .or_else(|| module.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("Untitled module")
                    .to_string(),
                week: module_week,
                items,
            });
        }
    }
    result
}

fn add_resource(
    resources: &mut Vec<Material>,
    index: &mut HashMap<String, usize>,
    resource: Material,
) {
    if let Some(&position) = index.get(&resource.resource_id) {
        let prior = &mut resources[position];
        let mut weeks = prior
            .weeks
            .clone()
            .unwrap_or_else(|| prior.week.map(|week| vec![week]).unwrap_or_default());
        if let Some(week) = resource.week
            && !weeks.contains(&week)
        {
            weeks.push(week);
        }
        if weeks.len() > 1 {
            prior.weeks = Some(weeks);
        }
        return;
    }
    index.insert(resource.resource_id.clone(), resources.len());
    resources.push(resource);
}

fn commons_resource(course_id: u32, module: &Value, item: &Value) -> Option<Material> {
    let data = data(item);
    let item_type = data
        .and_then(|data| data.get("item_content_type"))
        .or_else(|| item.get("content_type"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if item_type != "commons" {
        return None;
    }
    if let Some(owner) = data
        .and_then(|data| data.get("course_id"))
        .and_then(positive_id)
        && owner != course_id
    {
        return None;
    }
    let source_id = data
        .and_then(|data| data.get("item_id"))
        .and_then(positive_id)
        .or_else(|| item.get("content_id").and_then(positive_id))?;
    let media = data
        .and_then(|data| data.get("item_content_data"))
        .and_then(Value::as_object);
    let kind = content_type_name(
        media
            .and_then(|media| media.get("content_type"))
            .and_then(Value::as_str),
        media
            .and_then(|media| media.get("file_name"))
            .and_then(Value::as_str),
    );
    let reason = unavailable_reason(&[module.as_object(), item.as_object(), data]);
    let mut resource = Material {
        resource_id: format!("commons:{course_id}:{source_id}"),
        course_id,
        title: item
            .get("title")
            .and_then(Value::as_str)
            .or_else(|| {
                media
                    .and_then(|media| media.get("file_name"))
                    .and_then(Value::as_str)
            })
            .or_else(|| {
                data.and_then(|data| data.get("title"))
                    .and_then(Value::as_str)
            })
            .unwrap_or("Untitled material")
            .to_string(),
        week: week_of(module, data),
        weeks: None,
        source: "commons".to_string(),
        content_type: kind.clone(),
        available: reason.is_none(),
        download_supported: reason.is_none() && kind == "pdf",
        text_supported: reason.is_none() && kind == "pdf",
        created_at: data
            .and_then(|data| data.get("created_at"))
            .or_else(|| item.get("created_at"))
            .and_then(Value::as_str)
            .map(str::to_string),
        updated_at: data
            .and_then(|data| data.get("updated_at"))
            .or_else(|| item.get("updated_at"))
            .and_then(Value::as_str)
            .map(str::to_string),
        view_url: media
            .and_then(|media| media.get("view_url"))
            .and_then(Value::as_str)
            .and_then(viewer_url),
        post_title: None,
        board_id: None,
        post_id: None,
        attachment_id: None,
        download_url: None,
        unavailable_reason: reason,
    };
    if resource.available {
        resource.unavailable_reason = None;
    }
    Some(resource)
}

fn canvas_resource(
    course_id: u32,
    module: &Value,
    item: &Value,
    file: Option<&Value>,
) -> Option<Material> {
    let file = file.unwrap_or(&Value::Null);
    let source_id = item
        .get("content_id")
        .and_then(positive_id)
        .or_else(|| file.get("id").and_then(positive_id))?;
    let data = data(item);
    let media = data
        .and_then(|data| data.get("item_content_data"))
        .and_then(Value::as_object);
    let filename = file
        .get("display_name")
        .or_else(|| file.get("filename"))
        .and_then(Value::as_str)
        .or_else(|| item.get("title").and_then(Value::as_str));
    let kind = content_type_name(
        file.get("content-type")
            .or_else(|| file.get("content_type"))
            .and_then(Value::as_str)
            .or_else(|| {
                media
                    .and_then(|media| media.get("content_type"))
                    .and_then(Value::as_str)
            }),
        filename,
    );
    let reason =
        unavailable_reason(&[module.as_object(), item.as_object(), data, file.as_object()]);
    Some(Material {
        resource_id: format!("canvas:{course_id}:{source_id}"),
        course_id,
        title: item
            .get("title")
            .and_then(Value::as_str)
            .or(filename)
            .unwrap_or("Untitled file")
            .to_string(),
        week: week_of(module, data),
        weeks: None,
        source: "canvas".to_string(),
        content_type: kind.clone(),
        available: reason.is_none(),
        download_supported: reason.is_none(),
        text_supported: reason.is_none() && kind == "pdf",
        created_at: file
            .get("created_at")
            .or_else(|| item.get("created_at"))
            .and_then(Value::as_str)
            .map(str::to_string),
        updated_at: file
            .get("updated_at")
            .or_else(|| item.get("updated_at"))
            .and_then(Value::as_str)
            .map(str::to_string),
        view_url: None,
        post_title: None,
        board_id: None,
        post_id: None,
        attachment_id: None,
        download_url: None,
        unavailable_reason: reason,
    })
}

/// Build materials from LearningX board posts, including their attachments.
#[must_use]
pub fn board_materials(course_id: u32, posts: &[Value]) -> Vec<Material> {
    let mut resources = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for post in posts {
        let board_id = post.get("board_id").and_then(positive_id);
        let post_id = post.get("id").and_then(positive_id);
        let (Some(board_id), Some(post_id)) = (board_id, post_id) else {
            continue;
        };
        let post_title = post
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("LearningX post")
            .to_string();
        let week = week_from_title(&post_title);
        let created_at = post
            .get("created_at")
            .and_then(Value::as_str)
            .map(str::to_string);
        let updated_at = post
            .get("updated_at")
            .and_then(Value::as_str)
            .map(str::to_string);
        for attachment in post
            .get("attachments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(attachment_id) = attachment.get("id").and_then(positive_id) else {
                continue;
            };
            if !seen.insert((post_id, attachment_id)) {
                continue;
            }
            let filename = attachment
                .get("filename")
                .and_then(Value::as_str)
                .unwrap_or("attachment");
            let kind = content_type_name(None, Some(filename));
            let url = attachment
                .get("url")
                .and_then(Value::as_str)
                .map(str::to_string);
            resources.push(Material {
                resource_id: format!("board:{course_id}:{board_id}:{post_id}:{attachment_id}"),
                course_id,
                title: filename.to_string(),
                week,
                weeks: None,
                source: "learningx_board".to_string(),
                content_type: kind.clone(),
                available: url.is_some(),
                download_supported: url.is_some(),
                text_supported: url.is_some() && kind == "pdf",
                created_at: created_at.clone(),
                updated_at: updated_at.clone(),
                view_url: None,
                post_title: Some(post_title.clone()),
                board_id: Some(board_id),
                post_id: Some(post_id),
                attachment_id: Some(attachment_id),
                download_url: url,
                unavailable_reason: None,
            });
        }
    }
    resources
}

fn data(item: &Value) -> Option<&Map<String, Value>> {
    item.get("content_data").and_then(Value::as_object)
}

fn module_items(module: &Value) -> &[Value] {
    module
        .get("module_items")
        .or_else(|| module.get("items"))
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn is_canvas_file(item: &Value) -> bool {
    item.get("type")
        .or_else(|| item.get("content_type"))
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("file"))
}

fn positive_id(value: &Value) -> Option<u32> {
    match value {
        Value::Number(number) => number.as_u64().and_then(|value| u32::try_from(value).ok()),
        Value::String(text) => {
            if text.starts_with('0') || text.is_empty() {
                return None;
            }
            text.parse::<u32>().ok()
        }
        _ => None,
    }
}

fn week_of(module: &Value, data: Option<&Map<String, Value>>) -> Option<u32> {
    if let Some(data) = data {
        for key in ["week_position", "week", "position"] {
            if let Some(value) = data.get(key).and_then(positive_id) {
                return Some(value);
            }
        }
    }
    for key in ["week_position", "week", "position"] {
        if let Some(value) = module.get(key).and_then(positive_id) {
            return Some(value);
        }
    }
    let title = module
        .get("title")
        .or_else(|| module.get("name"))
        .and_then(Value::as_str)?;
    let pattern = Regex::new(r"([0-9]+)\s*주(?:차)?").ok()?;
    pattern
        .captures(title)
        .and_then(|captures| captures.get(1))
        .and_then(|value| value.as_str().parse().ok())
}

fn unavailable_reason(objects: &[Option<&Map<String, Value>>]) -> Option<String> {
    for object in objects.iter().flatten() {
        if object.get("opened").and_then(Value::as_bool) == Some(false)
            || object.get("published").and_then(Value::as_bool) == Some(false)
        {
            return Some("This material is not open".to_string());
        }
        if object.get("locked").and_then(Value::as_bool) == Some(true)
            || object.get("locked_for_user").and_then(Value::as_bool) == Some(true)
        {
            return Some("This material is locked".to_string());
        }
        if object
            .get("lecture_period_status")
            .and_then(Value::as_str)
            .is_some_and(|value| matches!(value, "not_open" | "locked" | "before_open" | "closed"))
            || object.get("content_id").and_then(Value::as_str) == Some("not_open")
        {
            return Some("This material is not open".to_string());
        }
    }
    None
}

fn viewer_url(value: &str) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("ocs.cau.ac.kr")
        || !url.path().starts_with("/em/")
        || url.path().len() <= 4
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(value.to_string())
}

fn content_type_name(value: Option<&str>, filename: Option<&str>) -> String {
    let mut value = value
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if value.is_empty() || value == "file" || value == "application/octet-stream" {
        value = filename
            .and_then(|filename| {
                std::path::Path::new(filename)
                    .extension()
                    .and_then(|extension| extension.to_str())
            })
            .map(|extension| match extension.to_ascii_lowercase().as_str() {
                "pdf" => "application/pdf".to_string(),
                "mp4" => "video/mp4".to_string(),
                other => other.to_string(),
            })
            .unwrap_or(value);
    }
    match value.as_str() {
        "application/pdf" => "pdf".to_string(),
        "video/mp4" => "mp4".to_string(),
        _ => value,
    }
}

fn normalized(value: Option<&str>) -> String {
    value
        .unwrap_or_default()
        .nfc()
        .collect::<String>()
        .to_lowercase()
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn module() -> Value {
        json!({
            "module_id": 7,
            "title": "9주차",
            "position": 9,
            "module_items": [{
                "module_item_id": 8,
                "title": "Scheduler",
                "content_type": "attendance_item",
                "content_id": 22,
                "content_data": {
                    "item_id": 22,
                    "course_id": 11,
                    "item_content_type": "commons",
                    "opened": true,
                    "lecture_period_status": "open",
                    "item_content_data": {
                        "content_id": "synthetic",
                        "content_type": "pdf",
                        "file_name": "Scheduler.pdf",
                        "view_url": "https://ocs.cau.ac.kr/em/synthetic"
                    }
                }
            }]
        })
    }

    #[test]
    fn parses_provider_qualified_ids() {
        let commons = parse_resource_id("commons:11:22").unwrap();
        assert_eq!(commons.source, "commons");
        assert_eq!(commons.course_id, 11);
        assert_eq!(commons.parts, vec![22]);
        let canvas = parse_resource_id("canvas:11:33").unwrap();
        assert_eq!(canvas.source, "canvas");
        assert_eq!(canvas.parts, vec![33]);
        let board = parse_resource_id("board:11:5:8:44").unwrap();
        assert_eq!(board.source, "board");
        assert_eq!(board.parts, vec![5, 8, 44]);
        for value in [
            "11:22",
            "commons:0:22",
            "commons:11",
            "x:11:22",
            "board:11:5",
        ] {
            assert!(parse_resource_id(value).is_err(), "{value}");
        }
    }

    #[test]
    fn parses_week_from_board_titles() {
        assert_eq!(week_from_title("제 11주차 강의자료"), Some(11));
        assert_eq!(week_from_title("제9주차 강의자료"), Some(9));
        assert_eq!(week_from_title("제 5강 강의자료"), Some(5));
        assert_eq!(week_from_title("강의자료"), None);
    }

    #[test]
    fn builds_board_attachment_materials() {
        let posts = vec![json!({
            "id": 298413,
            "board_id": 72832,
            "title": "제 11주차 강의자료",
            "created_at": "2026-11-12T14:51:54.000000Z",
            "attachments": [{
                "id": 336444,
                "filename": "제9강-1 국민소득의 측정.pdf",
                "url": "https://eclass3.cau.ac.kr/files/1/download"
            }]
        })];
        let materials = board_materials(148858, &posts);
        assert_eq!(materials.len(), 1);
        assert_eq!(materials[0].resource_id, "board:148858:72832:298413:336444");
        assert_eq!(materials[0].week, Some(11));
        assert_eq!(materials[0].content_type, "pdf");
        assert!(materials[0].download_supported);
        assert!(materials[0].text_supported);
    }

    #[test]
    fn catalog_builds_commons_and_canvas_resources() {
        let files = vec![json!({
            "id": 33, "display_name": "Notes.pdf",
            "content-type": "application/pdf", "size": 123
        })];
        let resources = material_catalog(11, &[module()], &files);
        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].resource_id, "commons:11:22");
        assert_eq!(resources[0].week, Some(9));
        assert!(resources[0].download_supported);
        assert_eq!(resources[1].resource_id, "canvas:11:33");
        assert!(resources[1].text_supported);
    }

    #[test]
    fn filters_week_title_and_type() {
        let resources = material_catalog(11, &[module()], &[]);
        assert_eq!(
            filter_materials(&resources, Some(9), Some("scheduler"), None).len(),
            1
        );
        assert_eq!(filter_materials(&resources, Some(8), None, None).len(), 0);
        assert_eq!(
            filter_materials(&resources, None, None, Some("pdf")).len(),
            1
        );
    }

    #[test]
    fn course_matching_normalizes_korean_and_term() {
        let course = json!({"name": "리눅스 시스템", "course_code": "CSE301", "term": {"id": 96, "name": "2026년 2학기"}});
        assert!(course_matches(
            &course,
            Some("리눅스시스템"),
            Some("2026년2학기")
        ));
        assert!(course_matches(&course, Some("cse301"), Some("96")));
        assert!(!course_matches(&course, Some("알고리즘"), None));
    }

    #[test]
    fn parses_term_keys() {
        assert_eq!(term_key("2026년 2학기"), Some((2026, 2)));
        assert_eq!(term_key("2021-1학기"), Some((2021, 1)));
        assert_eq!(term_key("2026학년도 2학기"), Some((2026, 2)));
        assert_eq!(term_key("2026년 여름학기"), None);
        assert_eq!(term_key("온라인 생명지킴이 교육"), None);
        assert_eq!(term_key_for_month(2026, 11), (2026, 2));
        assert_eq!(term_key_for_month(2026, 1), (2025, 2));
        assert_eq!(term_key_for_month(2026, 5), (2026, 1));
    }

    #[test]
    fn compact_modules_skips_empty_filtered_modules() {
        let modules = compact_modules(11, &[module()], Some(8), None, None);
        assert!(modules.is_empty());
        let modules = compact_modules(11, &[module()], Some(9), None, None);
        assert_eq!(modules.len(), 1);
        assert_eq!(modules[0].items.len(), 1);
    }
}
