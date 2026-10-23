// PR 草稿导出：Premiere Pro XML (xmeml v5)
// 复刻 CutMatch _write_pr_xml：每匹配段一个 clipitem，
// 序列内位置 = 对标时间，源入出点 = 原片时间，素材引用原视频绝对路径。

use serde_json::Value;

/// XML 文本转义
fn xml_escape(s: &str) -> String {
  s.replace('&', "&amp;")
    .replace('<', "&lt;")
    .replace('>', "&gt;")
    .replace('"', "&quot;")
    .replace('\'', "&apos;")
}

const PR_FPS: f64 = 25.0;

fn frames(seconds: f64) -> i64 {
  (seconds * PR_FPS).round() as i64
}

/// Tauri 命令：导出 Premiere Pro XML 草稿
#[tauri::command]
pub async fn video_export_pr_xml(
  segments: Vec<Value>,
  source_video: String,
  output_dir: String,
  draft_name: Option<String>,
  reference_duration: Option<f64>,
) -> Result<String, String> {
  if segments.is_empty() {
    return Err("没有可导出的匹配片段".to_string());
  }
  if !std::path::Path::new(&source_video).exists() {
    return Err(format!("原视频文件不存在: {source_video}"));
  }

  let draft_name = draft_name.unwrap_or_else(|| "PR导出".to_string());
  let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
  let file_name = format!("{}_{}.xml", draft_name, timestamp);
  let out_path = std::path::Path::new(&output_dir).join(&file_name);

  // 素材绝对路径（正斜杠）
  let abs_source = std::fs::canonicalize(&source_video)
    .map(|p| {
      p.to_string_lossy()
        .replace("\\\\?\\", "")
        .replace('\\', "/")
    })
    .unwrap_or_else(|_| source_video.replace('\\', "/"));
  let source_file_name = std::path::Path::new(&abs_source)
    .file_name()
    .map(|n| n.to_string_lossy().to_string())
    .unwrap_or_default();

  let mut xml = String::with_capacity(segments.len() * 1200 + 2048);
  xml.push_str(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
  xml.push('\n');
  xml.push_str(r#"<!DOCTYPE xmeml>"#);
  xml.push('\n');
  xml.push_str(r#"<xmeml version="5">"#);
  xml.push('\n');

  let seq_id = chrono::Utc::now().timestamp_millis();
  xml.push_str(&format!(r#"  <sequence id="sequence-{seq_id}">"#));
  xml.push('\n');
  xml.push_str(&format!("    <name>{}</name>\n", xml_escape(&draft_name)));

  // 对标总时长（秒）；未传则取最大 ref_end
  let total_dur = reference_duration.unwrap_or_else(|| {
    segments
      .iter()
      .map(|s| {
        s.get("sourceEnd")
          .or_else(|| s.get("source_end"))
          .and_then(|v| v.as_f64())
          .unwrap_or(0.0)
      })
      .fold(0.0f64, f64::max)
  });
  xml.push_str(&format!("    <duration>{}</duration>\n", frames(total_dur)));
  xml
    .push_str("    <rate>\n      <timebase>25</timebase>\n      <ntsc>FALSE</ntsc>\n    </rate>\n");
  xml.push_str("    <media>\n    <video>\n      <format>\n        <samplecharacteristics>\n          <width>1920</width>\n          <height>1080</height>\n        </samplecharacteristics>\n      </format>\n");
  xml.push_str("      <track>\n");

  for (idx, seg) in segments.iter().enumerate() {
    let ref_start = seg
      .get("sourceStart")
      .or_else(|| seg.get("source_start"))
      .and_then(|v| v.as_f64())
      .unwrap_or(0.0);
    let ref_end = seg
      .get("sourceEnd")
      .or_else(|| seg.get("source_end"))
      .and_then(|v| v.as_f64())
      .unwrap_or(0.0);
    let src_start = seg
      .get("targetStart")
      .or_else(|| seg.get("target_start"))
      .and_then(|v| v.as_f64())
      .unwrap_or(0.0);
    let src_end = seg
      .get("targetEnd")
      .or_else(|| seg.get("target_end"))
      .and_then(|v| v.as_f64())
      .unwrap_or(0.0);

    let clip = format!(
      "        <clipitem id=\"clip-{}\">\n          <name>{}</name>\n          <enabled>true</enabled>\n          <duration>{}</duration>\n          <rate>\n            <timebase>25</timebase>\n            <ntsc>FALSE</ntsc>\n          </rate>\n          <start>{}</start>\n          <end>{}</end>\n          <in>{}</in>\n          <out>{}</out>\n          <file id=\"file-{}\">\n            <name>{}</name>\n            <pathurl>{}</pathurl>\n          </file>\n        </clipitem>\n",
      idx + 1,
      xml_escape(&source_file_name),
      frames(src_end - src_start),
      frames(ref_start),
      frames(ref_end),
      frames(src_start),
      frames(src_end),
      idx + 1,
      xml_escape(&source_file_name),
      xml_escape(&abs_source),
    );
    xml.push_str(&clip);
  }

  xml.push_str("      </track>\n    </video>\n    </media>\n  </sequence>\n</xmeml>\n");

  std::fs::write(&out_path, xml).map_err(|e| format!("写入 PR XML 失败: {e}"))?;

  Ok(out_path.to_string_lossy().to_string())
}
