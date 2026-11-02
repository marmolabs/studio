//! Pages that keep up: a stream re-renders a fragment every second and sends it
//! only when it changed. Datastar morphs it into the page by its `id`.

use futures_util::Stream;
use std::future::Future;
use std::time::Duration;
use topcoat::{
    datastar::PatchElements,
    router::content::sse::{Event, KeepAlive, Sse},
};

pub const EVERY: Duration = Duration::from_secs(1);

/// The longest a page that draws only when something changed goes without
/// drawing, so the ages it shows ("5m ago") move on.
pub const QUIET_MAX: Duration = Duration::from_secs(30);

/// Streams a render's HTML whenever it differs from what was last sent. A
/// render that knows nothing changed since it last drew answers `None`,
/// without drawing; a failed render (studiod restarting, say) is skipped,
/// not fatal.
pub fn watch_changes<F, Fut>(render: F) -> Sse<impl Stream<Item = topcoat::Result<Event>> + Send>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = topcoat::Result<Option<String>>> + Send,
{
    let stream =
        futures_util::stream::unfold((render, String::new()), |(render, mut last)| async move {
            loop {
                tokio::time::sleep(EVERY).await;
                let Ok(Some(html)) = render().await else {
                    continue;
                };
                if html != last {
                    last.clone_from(&html);
                    let event: Event = PatchElements::new(html).into();
                    return Some((Ok(event), (render, last)));
                }
            }
        });
    Sse::new(stream).keep_alive(KeepAlive::new())
}
