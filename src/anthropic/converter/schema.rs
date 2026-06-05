//! JSON Schema 规范化

/// 规范化 JSON Schema，修复 MCP 工具定义中常见的类型问题
///
/// Claude Code / MCP 工具定义偶尔会出现 `required: null`、`properties: null` 等，
/// 导致上游返回 400 "Improperly formed request"（参见
/// `docs/troubleshooting/400-improperly-formed-request.md` TC-07）。
///
/// **Round 6 灰度结论**（2026-05-13）: `$schema` 和 `additionalProperties` 这两
/// 个字段的"缺失补默认"逻辑被证实**没有任何**避免 400 的实际作用 —— 实测三种
/// schema 形态（正常 / 空 properties / 缺 type）裁剪后上游均 200。troubleshooting
/// 文档归因的 400 触发点都跟这两个字段无关。
///
/// 因此改为：`$schema` 和 `additionalProperties` **缺失时不补**（与 kiro-cli 2.3.0
/// wire 字节对齐，每工具节约 ~80B 并消除 prefix-cache key 偏移）；仅在字段存在
/// 但形态非法时纠正。`type` / `properties` / `required` 三个字段是 TC-07 真正治理
/// 的目标，**保留**缺失补默认。
pub(super) fn normalize_json_schema(schema: serde_json::Value) -> serde_json::Value {
    let serde_json::Value::Object(mut obj) = schema else {
        // 整体非 object → 兜底空 schema。不补 $schema/additionalProperties。
        return serde_json::json!({
            "type": "object",
            "properties": {},
            "required": [],
        });
    };

    // $schema：缺失不补；存在但非合法非空字符串才纠正。
    if let Some(v) = obj.get("$schema")
        && v.as_str().is_none_or(|s| s.is_empty())
    {
        obj.insert(
            "$schema".to_string(),
            serde_json::Value::String("http://json-schema.org/draft-07/schema#".to_string()),
        );
    }

    // type（必须是字符串）—— 这是 MCP 异常的常见目标，保留缺失补默认。
    if obj
        .get("type")
        .and_then(|v| v.as_str())
        .is_none_or(|s| s.is_empty())
    {
        obj.insert(
            "type".to_string(),
            serde_json::Value::String("object".to_string()),
        );
    }

    // properties（必须是 object）—— 保留 null/缺失兜底。
    match obj.get("properties") {
        Some(serde_json::Value::Object(_)) => {}
        _ => {
            obj.insert(
                "properties".to_string(),
                serde_json::Value::Object(serde_json::Map::new()),
            );
        }
    }

    // required（必须是 string 数组）—— 保留 null/异常元素兜底。
    let required = match obj.remove("required") {
        Some(serde_json::Value::Array(arr)) => serde_json::Value::Array(
            arr.into_iter()
                .filter_map(|v| v.as_str().map(|s| serde_json::Value::String(s.to_string())))
                .collect(),
        ),
        _ => serde_json::Value::Array(Vec::new()),
    };
    obj.insert("required".to_string(), required);

    // additionalProperties：缺失不补；存在但非 bool/object 才纠正为 true。
    if let Some(v) = obj.get("additionalProperties")
        && !v.is_boolean()
        && !v.is_object()
    {
        obj.insert(
            "additionalProperties".to_string(),
            serde_json::Value::Bool(true),
        );
    }

    serde_json::Value::Object(obj)
}
