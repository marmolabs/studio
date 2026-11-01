//! A ticket's screenshots and clips from its capture runs. Files are served
//! only when studiod lists them for that ticket, so no other path is readable.

use studiod::api::{EvidenceFile, Request};
use topcoat::{
    context::{Cx, app_context},
    router::{Body, path_param, response::Response, route},
    view::{View, component, view},
};

use crate::{AppConfig, client, ui};

path_param!(id);
path_param!(attempt);
path_param!(name);

/// The ticket's evidence files, or none when studiod can't say.
pub async fn files(socket: &std::path::Path, ticket: &str) -> Vec<EvidenceFile> {
    client::call(
        socket,
        Request::Evidence {
            ticket: ticket.to_string(),
        },
    )
    .await
    .unwrap_or_default()
}

#[route(GET "/tickets/{id}/evidence/{attempt}/{name}")]
async fn evidence_file(cx: &Cx) -> topcoat::Result<Response> {
    let config = app_context::<AppConfig>(cx);
    let id = path_param::<Id>(cx).to_owned();
    let attempt: u32 = path_param::<Attempt>(cx).parse().unwrap_or(u32::MAX);
    let name = path_param::<Name>(cx).to_owned();
    let listed = files(&config.socket, &id).await;
    let Some(file) = listed
        .into_iter()
        .find(|f| f.attempt == attempt && f.name == name)
    else {
        return Ok(http::Response::builder()
            .status(http::StatusCode::NOT_FOUND)
            .body(Body::from("no such evidence"))?);
    };
    let bytes = tokio::fs::read(&file.path)
        .await
        .map_err(|e| client::error(format!("can't read {}: {e}", file.name)))?;
    Ok(http::Response::builder()
        .header(http::header::CONTENT_TYPE, content_type(&file.name))
        .header(http::header::CACHE_CONTROL, "no-cache")
        .header("x-content-type-options", "nosniff")
        .body(Body::from(bytes))?)
}

fn content_type(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        _ => "application/octet-stream",
    }
}

fn is_video(name: &str) -> bool {
    content_type(name).starts_with("video/")
}

fn is_art_file(name: &str) -> bool {
    name.starts_with("before-") || name.starts_with("after-")
}

/// The key in `<prefix><key>.<ext>`, or none when the name doesn't match.
fn art_key<'a>(name: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = name.strip_prefix(prefix)?;
    let key = rest.rsplit_once('.').map_or(rest, |(k, _)| k);
    (!key.is_empty()).then_some(key)
}

/// The latest attempt's captures, side by side.
#[component]
pub async fn screenshots(ticket: String, files: Vec<EvidenceFile>) -> topcoat::Result<impl View> {
    let latest = files.iter().map(|f| f.attempt).max();
    let shown: Vec<EvidenceFile> = files
        .into_iter()
        .filter(|f| Some(f.attempt) == latest && !is_art_file(&f.name))
        .collect();
    Ok(view! {
        if !shown.is_empty() {
            <section>
                <h2 class=(format!("mb-2.5 {}", ui::SECTION_TITLE))>
                    "Screenshots" if let Some(a) = latest { " · attempt " (a + 1) }
                </h2>
                <div class="grid grid-cols-1 gap-3 sm:grid-cols-3">
                    for file in shown {
                        let url = format!("/tickets/{ticket}/evidence/{}/{}", file.attempt, file.name);
                        <figure class=(ui::LIST)>
                            if is_video(&file.name) {
                                <video src=(url.clone()) controls="" class="block w-full"></video>
                            } else {
                                <a href=(url.clone()) target="_blank" rel="noopener">
                                    <img src=(url.clone()) alt=(file.name.clone()) loading="lazy" class="block w-full">
                                </a>
                            }
                            <figcaption class=(format!("px-3 py-1.5 font-mono text-[12px] {}", ui::MUTED))>(file.name)</figcaption>
                        </figure>
                    }
                </div>
            </section>
        }
    })
}

/// The latest attempt's shipped and new art, one row per key. A lone `after-`
/// shows "new file"; a lone `before-` has nothing to compare and is left out.
#[component]
pub async fn art_pairs(ticket: String, files: Vec<EvidenceFile>) -> topcoat::Result<impl View> {
    let latest = files.iter().map(|f| f.attempt).max();
    let url = |f: &EvidenceFile| format!("/tickets/{ticket}/evidence/{}/{}", f.attempt, f.name);
    let mut rows: Vec<(String, Option<String>, String)> = Vec::new();
    for after in files.iter().filter(|f| Some(f.attempt) == latest) {
        let Some(key) = art_key(&after.name, "after-") else {
            continue;
        };
        let before = files
            .iter()
            .find(|f| f.attempt == after.attempt && art_key(&f.name, "before-") == Some(key));
        rows.push((key.to_string(), before.map(url), url(after)));
    }
    rows.sort();
    Ok(view! {
        if !rows.is_empty() {
            <section>
                ui::section_head(title: "Images · shipped vs new", count: None, href: String::new(), label: String::new())
                <div class="space-y-3">
                    for (key, before, after) in rows {
                        <div class=(format!("p-3 {}", ui::CARD))>
                            <div class=(format!("mb-2 font-mono text-[12px] {}", ui::MUTED))>(key)</div>
                            <div class="grid grid-cols-2 gap-3">
                                <figure>
                                    if let Some(url) = before {
                                        <a href=(url.clone()) target="_blank" rel="noopener">
                                            <img src=(url.clone()) alt="shipped" loading="lazy" style="image-rendering: pixelated" class="block w-full bg-bg-2">
                                        </a>
                                        <figcaption class=(format!("pt-1.5 text-[12px] {}", ui::MUTED))>"shipped"</figcaption>
                                    } else {
                                        <div class=(format!("flex min-h-16 items-center justify-center bg-bg-2 text-[12px] {}", ui::MUTED))>"new file"</div>
                                    }
                                </figure>
                                <figure>
                                    <a href=(after.clone()) target="_blank" rel="noopener">
                                        <img src=(after.clone()) alt="new" loading="lazy" style="image-rendering: pixelated" class="block w-full bg-bg-2">
                                    </a>
                                    <figcaption class=(format!("pt-1.5 text-[12px] {}", ui::MUTED))>"new"</figcaption>
                                </figure>
                            </div>
                        </div>
                    }
                </div>
            </section>
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn file(attempt: u32, name: &str) -> EvidenceFile {
        EvidenceFile {
            attempt,
            name: name.into(),
            path: format!("/w/{name}"),
        }
    }

    async fn render(files: Vec<EvidenceFile>) -> String {
        let cx = Cx::default();
        ui::html(
            &cx,
            view! { cx => art_pairs(ticket: "STUD-1".to_string(), files: files) },
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_pair_shows_both_images_and_labels() {
        let html = render(vec![file(0, "before-hero.png"), file(0, "after-hero.png")]).await;
        assert!(html.contains("Images · shipped vs new"), "{html}");
        assert!(
            html.contains("/tickets/STUD-1/evidence/0/before-hero.png"),
            "{html}"
        );
        assert!(
            html.contains("/tickets/STUD-1/evidence/0/after-hero.png"),
            "{html}"
        );
        assert!(
            html.contains(">shipped<") && html.contains(">new<"),
            "{html}"
        );
        assert!(html.contains("image-rendering: pixelated"), "{html}");
        assert!(!html.contains("new file"), "{html}");
    }

    #[tokio::test]
    async fn a_lone_after_is_a_new_file() {
        let html = render(vec![file(0, "after-hero.png")]).await;
        assert!(html.contains("new file"), "{html}");
        assert!(html.contains("after-hero.png"), "{html}");
        assert!(!html.contains(">shipped<"), "{html}");
    }

    #[tokio::test]
    async fn no_pairs_render_nothing() {
        let html = render(vec![file(0, "title.png"), file(0, "before-hero.png")]).await;
        assert!(!html.contains("shipped"), "{html}");
    }

    #[tokio::test]
    async fn only_the_latest_attempt_is_paired() {
        let html = render(vec![file(0, "after-old.png"), file(1, "after-new.png")]).await;
        assert!(html.contains("after-new.png"), "{html}");
        assert!(!html.contains("after-old.png"), "{html}");
    }

    #[tokio::test]
    async fn screenshots_leave_out_art_files() {
        let cx = Cx::default();
        let files = vec![
            file(0, "title.png"),
            file(0, "before-hero.png"),
            file(0, "after-hero.png"),
        ];
        let html = ui::html(
            &cx,
            view! { cx => screenshots(ticket: "STUD-1".to_string(), files: files) },
        )
        .await
        .unwrap();
        assert!(html.contains("title.png"), "{html}");
        assert!(!html.contains("before-hero"), "{html}");
        assert!(!html.contains("after-hero"), "{html}");
    }

    #[test]
    fn content_types_follow_the_extension() {
        assert_eq!(content_type("title.PNG"), "image/png");
        assert_eq!(content_type("run.webm"), "video/webm");
        assert_eq!(content_type("notes"), "application/octet-stream");
        assert!(is_video("clip.mp4"));
    }
}
