# Packs

A pack is a folder of things you want to reuse across projects: hires, rules,
skills and a template for `.studio.toml`. The studio ships one, `packs/bevy-ios`.

## Layout

```text
my-pack/
  pack.toml
  hires/       *.md      hires, same format as the studio's hires/
  rules/       *.md      rules, same format as the studio's rules/
  skills/
    <name>/SKILL.md      instructions an agent can load on demand
  project.toml           template for a project's .studio.toml
```

Only `pack.toml` is required.

## pack.toml

```toml
name = "bevy-ios"
description = "Hires, rules and skills for a Bevy game that ships on iOS."
version = "0.1.0"
```

`name` is required: letters, digits, `-` and `_`. `description` and
`version` are optional. Other keys are errors.

## Use a pack

```sh
studio pack add path/to/my-pack      # adds it to studio.toml
studio project add ~/games/g --pack my-pack
```

`pack add` appends a `[[packs]]` entry to `studio.toml`. `--pack` copies the
pack's `project.toml` to the project's `.studio.toml`, but only if the repo
has none yet. Restart `studiod` to pick up a new pack.

## What a pack changes

- **Hires** load next to the studio's own, read-only. A studio hire with the
  same id wins.
- **Rules** load in the `pack/<name>` layer. Their ids look like
  `pack/my-pack/no-push`. Turn one off with `[rules] disable` in `studio.toml`.
- **Skills** are folders with a `SKILL.md` whose frontmatter has a `name` and a
  `description`. Agents list and load them with the `load_skill` tool. A skill
  in the studio's own `skills/` folder is found first.
- **project.toml** is a starting point. Edit the project's own copy; see
  [configuration](configuration.md).

## Write a pack

1. Make the folder and `pack.toml`.
2. Put hires in `hires/`. Give each a clear role and keep prompts short and
   specific.
3. Put rules in `rules/`. Prefer `ask` over `block` unless an action is never
   right. Always give a reason in the body.
4. Add skills for procedures an agent needs only sometimes, like a release
   checklist. Keep each one short.
5. Add a `project.toml` with sensible `verify` commands and any gates.
6. Add it to a test studio with `studio pack add`, then run `studio doctor`.
   It loads every hire and rule and reports problems.

## bevy-ios

The `bevy-ios` pack is for a Bevy game that ships on iOS.

- Hires: `engineer`, `producer`, `reviewer`, `artist`, `art-critic` and `store`.
- Rules: block `git push`; ask before changing signing files and entitlements
  or the Xcode project; tell agents to run the repo's own checks.
- Skills: `bevy-capture` (add or use a `--capture` mode) and
  `ios-release-checklist`.
- Template: `cargo fmt`, `clippy` and `test` as `verify`, a capture command, and
  gates that send art to the art critic and store copy to the reviewer.

`examples/starter-2d` is a small game that follows its conventions.
