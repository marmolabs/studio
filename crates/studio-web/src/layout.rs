use topcoat::{
    context::{Cx, app_context},
    router::{Slot, layout, request::uri},
    view::{View, view},
};

use crate::{AppConfig, client, statics, ui};

#[layout("/")]
pub async fn shell(cx: &Cx, slot: Slot<'_>) -> topcoat::Result<impl View> {
    let config = app_context::<AppConfig>(cx);
    // The shell still renders when studiod is down; the page says why.
    let status: client::Status = client::call(&config.socket, studiod::api::Request::Status)
        .await
        .unwrap_or_default();
    let path = uri(cx).path().to_owned();
    let nav = [
        ("/", "Board"),
        ("/office", "Office"),
        ("/goals", "Goals"),
        ("/asks", "Asks"),
        ("/budget", "Budget"),
        ("/hires", "Hires"),
        ("/rules", "Rules"),
    ];

    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"studio"</title>
                <link rel="stylesheet" href=(statics::STYLESHEET)>
                <script type="module" src=(statics::DATASTAR)></script>
            </head>
            <body class="min-h-screen bg-zinc-50 text-[13px] text-zinc-900 antialiased dark:bg-zinc-950 dark:text-zinc-100">
                <header class="sticky top-0 z-10 border-b border-zinc-300 bg-white/95 backdrop-blur dark:border-zinc-700 dark:bg-zinc-950/95">
                    <div class="mx-auto flex h-11 max-w-6xl items-center gap-3 px-3 sm:gap-5 sm:px-4">
                        <a href="/" class="shrink-0 font-semibold tracking-tight">"studio"</a>
                        <nav class="-mx-1 flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto px-1 [scrollbar-width:none] [&::-webkit-scrollbar]:hidden">
                            for (href, label) in nav {
                                let current = if href == "/" { path == "/" || path.starts_with("/tickets") } else { path.starts_with(href) };
                                <a
                                    href=(href)
                                    class=(if current {
                                        "inline-flex shrink-0 items-center gap-1.5 rounded-md bg-zinc-200 px-2 py-1 font-semibold text-zinc-950 dark:bg-zinc-700 dark:text-white"
                                    } else {
                                        "inline-flex shrink-0 items-center gap-1.5 rounded-md px-2 py-1 font-medium text-zinc-700 hover:bg-zinc-100 hover:text-zinc-950 dark:text-zinc-300 dark:hover:bg-zinc-800 dark:hover:text-white"
                                    })
                                    aria-current=(if current { "page" } else { "false" })
                                >
                                    (label)
                                    if label == "Asks" && status.asks > 0 {
                                        ui::count_badge(count: status.asks)
                                    }
                                </a>
                            }
                        </nav>
                        if status.running > 0 {
                            <span class="inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap text-[12px] text-zinc-700 dark:text-zinc-300">
                                <span class="size-2 rounded-full bg-emerald-500"></span>
                                (status.running) " running"
                            </span>
                        }
                        <a href="/budget" class="shrink-0 whitespace-nowrap text-[12px] tabular-nums text-zinc-700 hover:text-zinc-950 dark:text-zinc-300 dark:hover:text-white">
                            (ui::usd(status.spent_24h)) " of " (ui::usd(status.daily_usd))
                            <span class="hidden sm:inline">" today"</span>
                        </a>
                    </div>
                </header>
                <main class="mx-auto max-w-6xl px-3 py-4 sm:px-4">
                    <div id="notice" class="mb-3 empty:hidden"></div>
                    (slot)
                </main>
            </body>
        </html>
    })
}
