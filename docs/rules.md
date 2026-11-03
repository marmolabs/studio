# Rules

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
pattern: "sk-live-[0-9a-z]+"
---
Never commit live API keys.
```

When rules disagree, block beats ask beats allow.

## Gates on paths

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

## Command prefixes

A `bash:` gate that blocks or asks matches its prefix anywhere in the command,
so `bash:git push` also catches `cd app && git push`.

A `bash:` gate that allows is stricter. The command is split on `&&`, `||`,
`;`, `|` and `&`, and every part must start with an allowed prefix. A part
after a pipe may also be `head`, `tail`, `wc` or `grep` reading only the piped
output (no file names). Any `$`, backtick, redirect other than `2>&1`,
heredoc or subshell makes the command ask, because the split can't see into
it.

## Rules from corrections

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

When you send a task back, **Make it a rule for every task in** the project
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
