# Configuration

Everything is a file you can read and edit. The studio folder is
`~/Library/Application Support/studio` on macOS and `~/.config/studio` on
Linux. `--home <dir>` or `STUDIO_HOME` replaces it; the data folder is then
`<dir>/data`.

Unknown keys in `studio.toml` and `.studio.toml` are errors, so a typo is
caught when the file loads. Run `studio doctor` to check your files. The daemon
reads `studio.toml` and the hires again within a second of a change; a file
that no longer loads is reported in `studiod.log` and the last good one stays
in use. Runtimes and `[notify]` are read once, when the daemon starts, so
restart it (`studio down`, then `studio up`) after changing those. Rules apply
on the next run.

## studio.toml

### `[budget]` (required)

| Field | Default | Meaning |
| --- | --- | --- |
| `daily_usd` | none, required | Studio-wide cap over any rolling 24 hours. |
| `ticket_usd` | `2.0` | Cap for one ticket across all its runs. A hire's own cap can only lower it. |
| `run_usd` | none | Optional cap for one run. |
| `min_run_usd` | `0.05` | A run never starts with less allowance than this. |

### `[concurrency]` (required)

| Field | Default | Meaning |
| --- | --- | --- |
| `max_runs` | none, required | Agents running at once, from 1 to 9 (one per desk, set in the web UI's Company tab). `0` is an error: to pause the studio, run `studio close`; `studio open` starts it again. |

### `[[projects]]`

| Field | Default | Meaning |
| --- | --- | --- |
| `name` | required | Letters, digits, `-` and `_`. |
| `path` | required | The git repo. A relative path is read from the studio folder. |
| `integration_branch` | none | Branch work starts from. `studio project add` sets it to the branch checked out. |
| `daily_usd` | none | Cap for this project over any rolling 24 hours. |
| `watch_ci` | `false` | File a fix ticket when CI fails on the integration branch. Needs `gh`. |
| `lead` | none | The id of the hire who leads this project (see [Project leads](concepts.md#project-leads)). The hire needs `tools: [file_ticket]` to plan goals. `studio lead <project> <hire>` sets it (the hire must exist) and `studio lead <project>` clears it; the project's page in the web UI does the same. `studio doctor` flags a lead that isn't a hire. |

### `[[packs]]`

| Field | Default | Meaning |
| --- | --- | --- |
| `name` | required | Must match the pack's `pack.toml`. |
| `path` | required | The pack folder. A relative path is read from the studio folder. |

### `[rules]`

| Field | Default | Meaning |
| --- | --- | --- |
| `disable` | `[]` | Ids of pack rules to turn off, like `pack/my-pack/no-push`. Studio and project rules can't be disabled this way; delete the file. |

### `[prices."<model id>"]`

USD per million tokens, used to estimate spend during a run. Claude Code
reports the final cost itself. A model with no price and a runtime that
doesn't report cost won't run.

| Field | Default |
| --- | --- |
| `input_per_mtok` | required |
| `output_per_mtok` | required |
| `cache_read_per_mtok` | `0.0` |
| `cache_write_per_mtok` | `0.0` |

### `[goals]`

| Field | Default | Meaning |
| --- | --- | --- |
| `max_filed` | `8` | Most tickets one ticket's agent may file. |

### `[watchers]`

| Field | Default | Meaning |
| --- | --- | --- |
| `ci_minutes` | `10` | Minutes between CI checks. Values under 2 count as 2. |

### `[notify]`

| Field | Default | Meaning |
| --- | --- | --- |
| `desktop` | `false` | Send desktop notifications from the daemon. |

### Example

```toml
[budget]
daily_usd = 10.0
ticket_usd = 2.0

[concurrency]
max_runs = 2

[[projects]]
name = "my-app"
path = "/Users/me/code/my-app"
integration_branch = "master"
daily_usd = 5.0
watch_ci = true
lead = "manager"

[[packs]]
name = "my-pack"
path = "packs/my-pack"

[rules]
disable = ["pack/my-pack/no-push"]
```

## .studio.toml

This file sits in a project's root. Every field is optional.

| Field | Default | Meaning |
| --- | --- | --- |
| `verify` | `[]` | Commands that must pass after each attempt, each an array: `["cargo", "test"]`. Agents may also run them (program and subcommand, like `cargo test`) without asking. Each gets 30 minutes. |
| `protected` | `[]` | Path globs agents may not change without asking you. Same as an ask-level `protect` rule for this project. |
| `integration_branch` | none | Branch for this repo. |
| `capture` | `[]` | Command that runs the project and writes screenshots into `{out}`. It gets 30 minutes, building included. |
| `capture_paths` | `[]` | Changed-file globs that call for a capture. Empty means any change. |
| `gates` | `[]` | Critic gates, below. |
| `auto_land` | `false` | Land a ticket on its goal branch as soon as it is ready. A conflict sends it back to its agent to resolve, as landing by hand does. Never merges into the integration branch; that stays `studio merge`. |
| `env` | `{}` | Environment variables for the project's agents, checks and captures. `{repo}` in a value is the project's own checkout, so its worktrees can share a folder, such as one build folder instead of a build from scratch per ticket. `studio project add` sets one up for a Cargo project. |
| `release` | `[]` | Commands that publish a release, each an array, run in order once you approve it: `["./publish.sh"]`. They get `VERSION` in their environment, besides the project's `env`, and 30 minutes each. Empty means the studio can't release the project. See [Releases](#releases). |

Each `[[gates]]` entry:

| Field | Default | Meaning |
| --- | --- | --- |
| `name` | required | A label. |
| `paths` | required | Changed-file globs that send the work to this gate. |
| `hire` | required | The hire that judges the work. It can't be the ticket's own hire or run on the model that wrote the latest attempt: it falls back to the first of its `escalate` models the author didn't use, and the gate is skipped when there is none. A gate's critic is never routed tickets. |
| `prompt` | none | Extra instructions for the critic. |
| `rounds` | `3` | Most times the gate may send work back before it asks you. |
| `measure` | `[]` | Command run in the worktree on up to 12 changed files that match `paths`, with `{file}` in its arguments replaced by each repo-relative path: `["tools/pixel-lint", "{file}"]`. With no `{file}` it runs once. Deleted files and files reached through a symlink are skipped. The critic sees each command's output (cut to 2000 characters, 2 minute limit). A failing measurement never fails the ticket. The program can't be empty. |

Changed images (png, jpg, gif) that match `paths` are given to the critic beside their shipped version. The ticket page shows them as shipped vs new pairs.

Globs are checked when the file loads; a bad one is an error.

```toml
verify = [
  ["cargo", "fmt", "--check"],
  ["cargo", "test", "--locked"],
]
integration_branch = "master"
capture = ["cargo", "run", "--release", "--", "--capture", "{out}"]
capture_paths = ["src/**"]

[[gates]]
name = "design"
paths = ["static/img/**/*.png", "templates/**"]
hire = "design-critic"
prompt = "Judge the new screens against the screenshots."

[env]
CARGO_TARGET_DIR = "{repo}/.studio/target"
```

### Releases

A project with `release` commands can be released from the studio:
`studio release <project> <version>`, such as `studio release my-app 0.4`.

```toml
verify = [["cargo", "test", "--locked"]]
release = [
  ["./scripts/build-installers.sh"],
  ["./scripts/publish.sh"],
]
```

A release goes in four steps:

1. **Final checks.** The project's `verify` commands run on the tip of its
   integration branch, in a fresh checkout, with the project's `env` and the
   same 30 minute limit as any check.
2. **Your go-ahead.** The release waits for you and the studio tells you, as it
   does for anything that needs you: a desktop notification when `[notify]`
   is on, a bell on the project's sign, and a card in the For you tab.
   `studio release approve <project>` gives it, or Approve in the web UI.
   `studio release cancel <project>` stops a release that is still checking or
   waiting. Once you approve, it can't be stopped.
3. **The release commands** run in order in that same checkout, on the same
   commit, with `VERSION` set to the version you gave and the project's `env`.
4. **The tag.** The version is tagged `v<version>` (a version that already
   starts with `v` keeps it) on that commit, as you. The studio never pushes
   it; pushing the tag is yours to do.

A version uses letters, digits, dots and dashes only. A release is refused
when the project has no `release` commands, when one is already under way, or
when the tag exists. A release whose checks or commands fail stops there as
failed, with the command and the end of its output; nothing is tagged. A
release under way when studiod stops is failed with "studiod stopped".

`studio releases [project]` lists every release, newest first. Releases are
kept in the data folder's `releases` folder, one file per project. The web UI
shows each project's release under way, or one that failed or was cancelled in
the last day, and the latest that published. It lets you start a release only
for a project that has `release` commands.

## Hires

One file per hire in `hires/` (or a pack's `hires/`). The file name, without
`.md`, is the hire's id. YAML frontmatter, then the prompt.

| Field | Required | Meaning |
| --- | --- | --- |
| `name` | yes | Display name. |
| `role` | yes | Free text. `reviewer`, `producer` and `lead` have meaning to the studio. |
| `runtime` | yes | `claude-code`, the only runtime today. |
| `model` | yes | Model id or alias, like `sonnet`. |
| `escalate` | no | Stronger models to use after each failed attempt. The last one repeats. |
| `budget_per_ticket_usd` | no | Cap for one ticket on this hire. |
| `daily_usd` | no | Cap for this hire over any rolling 24 hours. |
| `tools` | no | Studio tools this hire may use. `file_ticket` lets it file tickets. |
| `user_mcp` | no | `true` gives this hire's runs your own Claude MCP servers as well. By default they get only the studio's tools. |
| `effort` | no | Reasoning effort: `low`, `medium`, `high`, `xhigh` or `max`. |

```markdown
---
name: Writer
role: writer
runtime: claude-code
model: sonnet
escalate: [opus]
daily_usd: 1.0
---
You write product copy and release notes that read like a person wrote them.
```

A pack's hires are read-only. A studio hire with the same id wins.

## Rules

One Markdown file per rule, in the studio's `rules/`, a pack's `rules/` or a
project's `.studio/rules/`. The kinds, fields, path globs, command prefixes
and drafting a rule from a correction are in [rules](rules.md).
