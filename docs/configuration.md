# Configuration

Everything is a file you can read and edit. The studio folder is
`~/Library/Application Support/studio` on macOS and `~/.config/studio` on
Linux. `--home <dir>` or `STUDIO_HOME` replaces it; the data folder is then
`<dir>/data`.

Unknown keys in `studio.toml` and `.studio.toml` are errors, so a typo is
caught when the file loads. Run `studio doctor` to check your files. The daemon
reads `studio.toml` when it starts; restart it after you change it. Rules
apply on the next run.

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
| `max_runs` | none, required | Agents running at once, at most 9 (one per desk in the office). `0` pauses the studio. |

### `[[projects]]`

| Field | Default | Meaning |
| --- | --- | --- |
| `name` | required | Letters, digits, `-` and `_`. |
| `path` | required | The git repo. A relative path is read from the studio folder. |
| `integration_branch` | none | Branch work starts from. `studio project add` sets it to the branch checked out. |
| `daily_usd` | none | Cap for this project over any rolling 24 hours. |
| `watch_ci` | `false` | File a fix ticket when CI fails on the integration branch. Needs `gh`. |

### `[[packs]]`

| Field | Default | Meaning |
| --- | --- | --- |
| `name` | required | Must match the pack's `pack.toml`. |
| `path` | required | The pack folder. A relative path is read from the studio folder. |

### `[providers.<name>]`

Names an extra runtime that hires can use in `runtime:`. `claude-code` is
always available without an entry.

| Field | Default | Meaning |
| --- | --- | --- |
| `kind` | required | Only `claude-code` today. |

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

## Hires

One file per hire in `hires/` (or a pack's `hires/`). The file name, without
`.md`, is the hire's id. YAML frontmatter, then the prompt.

| Field | Required | Meaning |
| --- | --- | --- |
| `name` | yes | Display name. |
| `role` | yes | Free text. `reviewer` and `producer` have meaning to the studio. |
| `runtime` | yes | `claude-code`, or a provider name from `studio.toml`. |
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

One file per rule in `rules/` (studio-wide), a pack's `rules/`, or a project's
`.studio/rules/`. The file name, without `.md`, is the rule's id, shown as
`studio/<name>`, `pack/<pack>/<name>` or `project/<project>/<name>`. The body
is the reason shown to the agent and to you.

| Field | Required | Meaning |
| --- | --- | --- |
| `id` | no | If set, it must match the file name. |
| `kind` | yes | `protect`, `gate`, `lint`, `check`, `guide` or `review`. |
| `level` | yes | `block`, `ask`, `warn` or `allow`. |
| `when` | no | Limits where the rule applies, below. |
| `pattern` | lint | A regex that must not appear in a change. |
| `command` | check | A command that must pass, as an array. |
| `action` | gate | A tool name like `Bash`, several like `Read\|Grep`, or a command prefix like `bash:git push`. |

What each kind needs: `protect` needs `when.paths`; `lint` needs `pattern`;
`check` needs `command`; `gate` needs `action`; `guide` and `review` need a
non-empty body.

`when` takes any of these lists. An empty list or `"*"` matches everything.

- `projects`: project names.
- `paths`: globs, relative to the repo root.
- `hires`: hire ids.
- `roles`: hire roles.

```markdown
---
kind: lint
level: block
pattern: "ca-app-pub-[0-9]+"
---
Never commit live ad unit IDs.
```

When rules disagree, block beats ask beats allow.

### Gates on paths

A gate with `when.paths` covers only calls on those paths. An allow needs every
path the call touches to match; a block or ask needs any, and also covers calls
that name no path. Paths outside the project are absolute, and `~/` means your
home folder:

```markdown
---
kind: gate
level: allow
action: "Read|Grep|Glob"
when:
  paths: ["~/.cargo/registry/**"]
---
Agents may read dependency sources.
```

### Command prefixes

A `bash:` gate that blocks or asks matches its prefix anywhere in the command,
so `bash:git push` also catches `cd app && git push`.

A `bash:` gate that allows is stricter. The command is split on `&&`, `||`,
`;`, `|` and `&`, and every part must start with an allowed prefix. A part
after a pipe may also be `head`, `tail`, `wc` or `grep` reading only the piped
output (no file names). Any `$`, backtick, redirect other than `2>&1`,
heredoc or subshell makes the command ask, because the split can't see into
it.

### Rules from corrections

When you deny an ask, the web UI offers **Make this a rule**. It opens the
rules editor with a draft you can change before you save it:

- A denied command becomes a `bash:` gate that blocks that command's prefix in
  the project. A command with `&&`, `|` or similar becomes a gate that asks for
  the tool instead.
- A denied file edit or read becomes a `protect` rule that asks before that
  path is touched.
- Any other tool becomes a gate that asks for it.
- An agent's question is not a permission, so denying one offers no rule.
- Your note on the denial is the rule's reason. If you left none, the draft
  tells you to write one.

When you send a ticket back, **Make it a rule for every ticket in** the project
opens a draft `guide` rule with your feedback as its text.

On the command line, `studio deny <ask> -m "why" --rule` and
`studio changes <ticket> "feedback" --rule` save the draft as a new file in
`rules/`. An existing file is never replaced; a second draft with the same
name gets `-2`, `-3` and so on. `studio deny --rule` on a question is an error.

A rule can be tried before you save it. The **Test** button in the rules
editor, and `studio rule test <name-or-path> [--since 7d]`, replay it against
the tool calls in the run logs since then (`<n>d` or `<n>h`; the default is 7
days) and list the ones it would have acted on. Only `gate`, `protect` and
`lint` rules act on tool calls, so only those can be tested. A rule with a bad
regex or path glob is refused, as when saving.
