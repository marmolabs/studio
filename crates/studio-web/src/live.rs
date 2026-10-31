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

/// Streams `render()`'s HTML whenever it differs from what was last sent.
/// A failed render (studiod restarting, say) is skipped, not fatal.
pub fn watch<F, Fut>(render: F) -> Sse<impl Stream<Item = topcoat::Result<Event>> + Send>
where
    F: Fn() -> Fut + Send + 'static,
    Fut: Future<Output = topcoat::Result<String>> + Send,
{
    let stream =
        futures_util::stream::unfold((render, String::new()), |(render, mut last)| async move {
            loop {
                tokio::time::sleep(EVERY).await;
                let Ok(html) = render().await else {
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
