# Web UI design

The studio is a company, and the web UI is how the owner runs it: a team
sidebar, with one lead for each project, and a page for each thing the owner
does. The reference for the team pages (HQ, a lead's conversation, Look closer,
the office, New project and the studio settings) is the gallery's pictures
(`examples/gallery/`, below). The reference for the task page, the hires page
and the rules page is `prototype.html`: open it in a browser (sidebar links
switch pages; the moon button switches theme) or render it with Quick Look.
Like the app, it has no script of its own: links and `:target` pick the page,
popovers open the dialogs, and a checkbox flips the theme. Its CSS is the design
system: take the values, spacing and structure from it. Its data is sample data;
the app shows only real data from studiod. Its office page is not a reference:
the team pages replaced it.

When a page and its pictures disagree, fix the page; when the design changes on
purpose, render the gallery again so the pictures stay true. The owner approved
the mockups the team pages follow; the gallery is how they look with real
studios.

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
  4.5:1 on its background in both themes; keep it that way. Each project has a
  colour of its own, used for its lead's progress bar and its floor.
- Type: IBM Plex Sans (one variable file, weights 400 to 600) and IBM Plex Mono
  400 and 500, from `vendor/fonts/` (SIL OFL 1.1, `OFL.txt`), served by the app
  and declared with `@font-face`, with system fallbacks. Body 14px/20px; tables
  and secondary text 13px or 12.5px; labels 12px. Mono only for ids, branches,
  paths, commands, models and code. Numbers use tabular figures.
- Icons: Lucide (ISC, `vendor/lucide/LICENSE`), 16px, stroke 1.75, from the
  sprite in `vendor/lucide/icons.svg`.
- Faces: a hire is a small portrait in a circle (24, 30, 40 or 48px): background,
  shirt, neck, head, hair, eyes and a smile, drawn as an inline SVG. Skin, hair
  and the hair style come from the hire id, the same as the floor's figures
  have them; the shirt is the role's colour, and a lead's is its project's. The
  owner wears a dark jacket and a white collar.
- Spacing: 4px grid; radius 7px buttons, 10px containers, 12px cards.

## Words

Plain and short, from the owner's side. Say task (not ticket), done or added
(not landed), ship (not merge) and the lead (not router, approver or producer).
No ids in a sentence where a title reads better; an id may show small in
metadata.

## The frame

Every team page sits in the same frame: the team sidebar (288px) beside the
page's own main area.

- The sidebar: the studio's name and a gear to the studio settings; **HQ** with
  a count of what waits for the owner; **Your leads**, each with its face, name,
  its project, its phase in a word (Building, Testing, "v1.2 waits for you",
  Live v1.0, Setting up...), a thin bar in the project's colour and a yellow
  count when something waits for the owner; **New project** ("Hire a lead for
  it"); at the bottom a small live picture of the office floor, which links to
  the office, and "Spent today $6.59 of $10" with a thin bar.
- On a phone the sidebar stacks above the page (the leads in a row that
  scrolls), the conversation's rail is hidden and the sheet is full width.
  Nothing scrolls the page sideways at 390px.
- Every team page keeps itself up to date from one stream; what the owner has
  typed or opened stays as it is.

## Pages

- **HQ** (`/`): the date, "Good morning" (afternoon, evening: the machine's
  clock) and a "3 need you" pill. One line for each lead: its face, name,
  project, time and its standup in a sentence or two. Under a lead's line, the
  cards that wait for the owner in that project: questions with Allow and Deny,
  work ready to ship with Ship and Look closer, a release that waits, with
  Publish and Not yet. Leads with something for the owner come first. A footer
  line gives the day's totals: tasks done, sent back and spent. A project with
  no lead has the day's facts for its words and, under them, a quiet row, "No
  lead yet", with the link "Give {project} a lead" to its conversation.
- **A conversation** (`/leads/{project}`): a header with the lead's face, name,
  "Leads shop", the phase pill and the milestone's progress. The thread, newest
  at the bottom: the lead's messages (face, name, time and short words) with
  cards under them, and the owner's messages as right-aligned bubbles. Cards
  come from stored messages (a plan, what the lead did, a setup) and from the
  live state (questions that wait, work ready to ship, a release that waits,
  today's numbers), merged by time. A composer at the bottom has quick chips
  ("How's it going?", "What's next?", "Plan something new") and "Message Nora";
  a lead that has not looked at the project, and has said nothing yet, also
  gets a chip in blue, "Ask Nora to look at how shop runs", which has it read
  the project and say how it runs, as New project does. While the lead works on
  a reply, the thread says "Nora is replying...". The header has a small
  **Change lead** menu: another lead, a new hire or no lead. A project without
  a lead has a card in place of the composer: "{project} has no lead yet. A
  lead plans the work, hands it out and answers the team's routine questions."
  with two ways to give it one, **Hire a lead** (a name, suggested, on a strong
  model) and **Pick one you have** (left out when no hire has the role lead);
  the page turns into the conversation with the new lead as it is named. A rail
  on the right, "In shop now", shows each person at work (face, name, what they
  are doing and for how long, and a message button on a running agent), the
  milestone with its tasks as ticks, and the project's spend today against its
  cap.
- **Look closer** (`?look={id}` on a conversation; the id is a goal's plan task
  or a ready task): a sheet that slides over the conversation from the right
  (600px, the rest dimmed). The work's name, project, task count and lines
  added and removed, with a close button; the lead's sentence on it; three proof
  tiles (the checks, with how many passed; the review; protected paths touched,
  and that the owner allowed it); the tasks with their lines; the files, the
  first one opened with its diff and the rest folded. The footer has **Ship to
  main**, **Send back with a note** (opens a field), "Try it first" with the
  command to switch to the branch and a copy button, and "Open in editor".
- **The office** (`/office`): the company floor inside the same frame. Picking a
  sign or a person floats a card for that project's lead (face, name, "Leads
  shop · Building", a line, a count, **Talk to Nora** and "What's ready"). A hint
  says "Click anyone to talk to them". `?pick=projects/shop` picks a project
  as a click does, and `?turn=` turns the floor. The floor is drawn from the
  studio's real state: a team area for each project, the owner's glass office,
  the checks lab, the review room, the lounge and an empty dashed area for the
  next project; over each team area hangs a sign with the phase, the milestone,
  and who is on it. Dark is the office at night, light the same office by day.
- **New project** (`/new`): a short form, the folder and the lead (a new hire
  with a name the studio suggests, on a strong model, or one you already have).
  Submitting opens the conversation, where the lead greets the owner and shows
  **How {project} runs** as a card: the checks, each tried by the studio with its
  result ("112 pass", "failed"), screenshots, release steps and where they were
  found, "Ask you first" paths and the daily budget, with **Use these**,
  **Change something** and **See the file**; then it offers a first milestone
  (**Plan it**, **Not yet**). While the lead reads or the studio tries the
  checks, the card says so; a setup that failed says why and has **Try again**.
  The gallery shows the card in each of its states.
- **Studio settings** (`/studio`): the money limits, the desks (how many agents
  work at once), the hires with Hire someone, a link to the rules and open or
  close the office.
- **Task**: breadcrumb; title, who filed it and from which goal; the detail as
  Markdown; the ask card when there is one; Activity as one feed of studio
  events, agent messages (Markdown), grouped tool calls with a one-line summary
  and per-call results, and asks. Right rail: status, hire and model, goal,
  project, branch, try n of 3, spend of the task's cap, time; progress steps
  for this task's real stages (checks, each gate that applies, review, ship);
  changed files with +/-. Evidence and image pairs fit in the main column.
  Finished tasks state the outcome and offer only actions that still apply.
- **Hires**: the team as a table (hire with role, model and escalation, effort,
  what it is on now, what it did today, spend today, Edit), then the studio's own
  hires (approver, router) with what they did today.
- **Rules**: rules grouped by where they come from, each with kind and level;
  the selected rule's file in an editor beside the list (the first rule by
  default); Delete as a quiet button in the editor footer, with a confirmation.

Addresses from older versions lead to the page that took them over: `/asks`,
`/board` and `/goals` to HQ, `/budget` to the studio settings, and a link to a
project to its lead's page.

## How, without our own JavaScript

The app stays Rust + Topcoat + Datastar + Tailwind.

- Menus and forms: the HTML `popover` attribute and `popovertarget` buttons;
  forms submit through the existing endpoints.
- Theme: the switch is a small form that sets a `theme` cookie (`light`,
  `dark`, or none for the system); the server puts `data-theme` on `<html>`.
- Grouped tool calls: `<details>`.
- Picking on the office floor: a Datastar `$sel` signal; the floor and the
  lead's card come from `/team/office/scene`.
- Live data keeps flowing through the one stream of the team pages.

## Real data only

Every number and label comes from studiod. Where the daemon doesn't have
something a page needs, add it to the API with a test. If it can't be had
honestly, leave the element out; never show a made-up or placeholder value.

What a person did today counts under the person who did it. A project's lead
is credited with the goals it planned and the calls and assignments it
decided; the studio's approver and router are credited only with what they
decided themselves. The daemon's notes in a run log say who decided (a `by`
field beside the note), and the activity feed reads only the note's words.

## Checking your work

`cargo run -q -p studio-web --example gallery -- <dir>` renders every page from
real studios in temporary homes, with scripted agents doing the work. A lead's
turns can't run there, so the shop's conversation and the leads' standups are
written into the studio before it is served; every other card is the studio's
own. It writes each page at 1024px, 1440px and 390px (phone), and some in the
light theme. In a sandbox it uses Quick Look, which lays pages out in a square
and shows animations still; look at the pages in a browser at 1440x900 too.

The pictures to check, against the descriptions above:

- `hq*`: HQ of a busy studio, an idle one, one with a release that waits, one
  with a project that has no lead (`hq-no-lead`) and an empty one.
- `lead-*`: a conversation for each phase of a project (the shop's has the
  owner's messages, the lead's replies, a plan and an action as cards), and
  one for a project that has no lead (`lead-no-lead`), with the card that
  gives it one.
- `look-task` and `look-goal`: Look closer on ready work and on a ready goal.
- `office`, `office-shop` and `office-closed`: the floor, with the shop picked,
  and with the office closed.
- `new` and `new-first`: New project, with leads and without.
- `setup-reading`, `setup-checking`, `setup-ready`, `setup-failed` and
  `setup-accepted`: a project being set up, in each state.
- `studio` and `studio-empty`: the studio settings.
- `ticket-*`, `hires` and `rules*`: the task, hires and rules pages.
