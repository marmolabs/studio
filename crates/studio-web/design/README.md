# Web UI design

`prototype.html` is the reference for every page except the Office: open it in
a browser (sidebar links switch pages; the moon button switches theme) or render
it with Quick Look (below). Like the app, it has no script of its own: links and
`:target` pick the page and the panel, popovers open the dialogs, and a checkbox
flips the theme. Build the app to look and behave like it. Its CSS is the design
system: take the values, spacing and structure from it. Its data is sample data;
the app shows only real data from studiod. The Office is the exception: its
reference is the gallery's office screenshots (`examples/gallery/`, below).

## Look

A calm, dense tool in the spirit of Linear: neutral chrome, colour only for
state, rows and tables rather than stacks of cards, one bordered container per
group. Dark is the default; light follows `prefers-color-scheme`, and the owner
can force either (below).

- Colours: copy the four token blocks at the top of the prototype's `<style>`
  (dark on bare `:root`, light under `prefers-color-scheme: light` guarded by
  `:root:not([data-theme="dark"])`, then `:root[data-theme="light"]` and
  `:root[data-theme="dark"]`) into `src/app.css` and expose them to Tailwind
  with `@theme inline`. Neutrals are Radix Slate, states are Radix Amber (needs
  you), Blue (running), Grass (done), Red (failed). Every text colour passes
  4.5:1 on its background in both themes; keep it that way.
- Type: IBM Plex Sans (one variable file, weights 400 to 600) and IBM Plex Mono
  400 and 500, from `vendor/fonts/` (SIL OFL 1.1, `OFL.txt`), served by the app
  and declared with `@font-face`, with system fallbacks. Body 14px/20px; tables
  and secondary text 13px or 12.5px; labels 12px; page title 15px semibold in
  the top bar; a ticket title 26px semibold with -0.02em tracking. Mono only for
  ids, branches, paths, commands, models and code. Numbers use tabular figures.
- Icons: Lucide (ISC, `vendor/lucide/LICENSE`), 16px, stroke 1.75, from the
  sprite in `vendor/lucide/icons.svg`, inlined once per page and used as
  `<svg class="i"><use href="#i-NAME"/></svg>`. State glyphs (queued, needs
  you, held, ready, landed, failed, cancelled) are the `s-*` symbols in the
  prototype; running is the CSS conic "prog" ring with `--p` for the step.
- Spacing: 4px grid; page padding 28px 32px; 28px between sections; rows 56px
  for two lines, 44px for one; radius 7px buttons, 10px containers, 12px ask
  cards.

## Shell

Sidebar (232px) on every page but the Office: the brand, then Office; a
"Studio" label; Hires, Rules. At its foot: running and queued counts with a
pulsing dot, today's spend of the daily limit with a meter (it leads to the
office's Money tab), and the theme switch. The Office has no sidebar: it draws
its own top bar (see Pages). Top bar (56px, sticky, translucent): title or
breadcrumb, then page actions on the right, at most one primary button. Content in a
centred column up to 1200px. Below 760px the sidebar becomes a top bar with the
brand, running count and spend, and a horizontally scrolling nav that starts
with Office; rails stack under the main column; nothing scrolls the page
sideways at 390px. Leave out the prototype's "Prototype" and "Sample data"
labels.

## Pages

- **Office** (the home page, `/`): the whole studio as one isometric floor on
  a page of its own, with no sidebar. Its visual reference is the gallery's
  office screenshots (`examples/gallery/`), not the prototype. A top bar holds
  the brand, a strip of projects that focuses one (the rest of the floor
  fades), the figures (need you, at work, shipped today), the money meter, Ticket,
  New goal and the theme switch. Under it, the floor: an SVG drawn from the
  studio's real state, with a label pinned over each ticket at a desk (an id,
  and its title on hover or when picked), plus view controls (zoom out, Fit,
  zoom in, "What's what"), two corners (a Projects tracker and a Live feed) and
  a side panel with the tabs Needs you, Projects, Team, Money, Rules and Log.
  - Every object on the floor is a control and opens its own panel (`elements/<key>`):
    the whiteboard (goals), the team board (hire), the posters (house rules), the
    racks (each project's checks), the empty desk (desks: how many agents work at
    once), the lounge (who is free), the coffee machine (close or reopen the
    office; nothing new starts while it is closed), the trophies (from the
    studio's history), the review room (reviewers and critics), the crates
    (shipped today), the returns (failed work), the front desk (approver and
    router), the queue (up next) and the safe (budget and limits).
  - A ticket at a desk, a hire and a goal open theirs: a ticket's panel follows its
    state (the ask with Allow and Deny, what the agent is doing, the checks and
    review, changed files with Land, a held ticket's reason with Resume,
    Cancel); a hire's says what it did and what it is on, with a level from
    landed work; a goal's lists what it filed, with Merge once it is done.
  - The Projects tab ends with History, what its other rows leave out, newest
    first: goals that were merged and tickets that finished more than a day
    ago. Each is a row like the tab's task rows and opens its panel. Twenty
    show at first, and "Show more" adds twenty.
  - Adding a project asks studiod what it would add, then shows the name, the
    full folder, the branch and the checks, with "Add it" and Cancel; only
    "Add it" adds anything.
  - Links: `/?tab=money` opens a tab, `/?sel=tickets/SHOP-3`, `hires/<id>`,
    `goals/<id>` or `elements/coffee` open a panel. An unknown tab opens Needs
    you; an unknown element shows the tabs.
  - Dark is the office at night, light the same office by day. The page, its
    figures, labels and panel stay live. Below 900px the floor fills the width,
    the corners and the panel follow it, and only the labels that need you are
    shown on the floor.
- **Ticket**: breadcrumb; title, who filed it and from which goal; the detail
  as Markdown; the ask card when there is one; Activity as one feed of studio
  events, agent messages (Markdown), grouped tool calls with a one-line summary
  and per-call results, and asks. Right rail: status, hire and model, goal,
  project, branch, try n of 3, spend of the ticket cap, time; progress steps
  for this ticket's real stages (checks, each gate that applies, review, land);
  changed files with +/-. Evidence and image pairs fit in the main column.
  Finished tickets state the outcome and offer only actions that still apply.
  Pause and Cancel live in the "more" menu.
- **Hires**: the team as a table (hire with role, model and escalation,
  effort, what it is on now, what it did today, spend today, Edit), then the
  studio's own hires (approver, router) with what they did today.
- **Rules**: rules grouped by where they come from, each with kind and level;
  the selected rule's file in an editor beside the list (the first rule by
  default); Delete as a quiet button in the editor footer, with a confirmation.

## How, without our own JavaScript

The app stays Rust + Topcoat + Datastar + Tailwind.

- Menus and the New ticket / New goal forms: the HTML `popover` attribute and
  `popovertarget` buttons, styled as the prototype's dialog; forms submit
  through the existing endpoints.
- Showing what adding a project would do: the check's answer goes into a
  signal, and the panel swaps its form for a card that reads the signal.
- Theme: the switch is a small form that sets a `theme` cookie (`light`,
  `dark`, or none for the system); the server puts `data-theme` on `<html>`.
- Grouped tool calls: `<details>`.
- Picking on the office floor: a Datastar `$sel` signal; the panel comes from
  `/office/panel` and the stream re-renders it with the floor.
- Live data keeps flowing through the existing Datastar streams.

## Real data only

Every number and label comes from studiod. Where the daemon doesn't have
something a page needs (for example spend per day or per model, or counts of
the approver's decisions), add it to the API with a test. If it can't be had
honestly, leave the element out; never show a made-up or placeholder value.

## Checking your work

`cargo run -q -p studio-web --example gallery -- <dir>` renders every page from
real studios in temporary homes, with scripted agents doing the work. In a
sandbox it uses Quick Look, which lays pages out at 1024px;
the gallery also writes 1440px and 390px (phone) versions. Compare them with
the prototype rendered the same way; the Office has no prototype, so check
its screenshots (the floor, each panel tab, both themes, phone width) against
the description under Pages.
