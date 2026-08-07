# Session templates

Start a Zellij session whose tabs are already laid out the way you work, from a
template you name once and reuse.

## Problem

Zellaude's custom states open a configured command grid in a **new tab** of an
already-running session (`Ctrl+t`, `Shift+n`). There is no equivalent at session
creation: a fresh `zellij -s work` gives one empty shell, and rebuilding the same
four tabs by hand every morning is the friction this removes.

Zellij already knows how to start a session from a layout — `zellij -s work -n
<layout>` reads `~/.config/zellij/layouts/<layout>.kdl`. What is missing is a way
to describe that layout in the JSON file users already edit, instead of writing
KDL by hand.

## Approach

Zellaude compiles session templates from `zellaude.json` into layout files that
Zellij reads natively.

```
~/.config/zellij/plugins/zellaude.json      (the user edits this)
        │
        │  the status bar compiles it on load
        ▼
~/.config/zellij/layouts/<name>.kdl         (Zellij reads this)
        │
        ▼
zellij -s work -n <name>
```

No new command, no shell wrapper, no interception of the `zellij` binary. Adding
a `--custom` flag to `zellij` is not possible — its argument parser rejects
unknown flags, and shadowing the binary would mean either a PATH shim or an
entry in the user's shell rc, neither of which a plugin should install.

Compilation happens in the plugin rather than in a shell script so that KDL
generation exists in exactly one place, alongside the validation that custom
states already use. The status bar runs in every session, so the compiler runs
often enough to keep the layout files current. The rule matches custom states:
**edit the JSON, reload the plugin.**

## Configuration

`session_templates` lives in `~/.config/zellij/plugins/zellaude.json` beside the
existing `custom_states` key. Neither reads the other.

```json
{
  "session_templates": [
    {
      "name": "work",
      "tabs": [
        { "name": "git", "commands": ["lazygit", "btop"] },
        { "name": "claude", "commands": ["claude"] },
        { "name": "editor", "cwd": "src", "commands": ["nvim"] },
        { "name": "shell" }
      ]
    }
  ]
}
```

### Template fields

| Field | Required | Meaning |
|-------|----------|---------|
| `name` | yes | Layout name; becomes the filename and the argument to `zellij -n` |
| `cwd` | no | Default directory for every tab |
| `tabs` | yes | At least one tab |

### Tab fields

| Field | Required | Meaning |
|-------|----------|---------|
| `name` | no | Tab label; Zellij names it automatically when absent |
| `cwd` | no | Overrides the template `cwd` for this tab |
| `commands` | no | One command per pane; absent means a plain shell tab |
| `width`, `height` | no | Pane grid; defaults to `commands.len()` × 1 |
| `focus` | no | Start on this tab; the first tab wins when absent |

`width` and `height` describe how `commands` is arranged, so giving either
without `commands` is an error. As with custom states, `commands` may be shorter
than `width × height` — the leftover cells open as plain shell panes — but never
longer, and the grid may not exceed 64 panes.

Commands run through `sh -lc`, exactly as custom-state commands do. Template
configuration is trusted local shell code.

### Working directories

Zellij resolves a layout's directories against the directory the session was
started from. Verified against Zellij 0.44.1:

| `cwd` | Result when started from `~/repo` |
|-------|-----------------------------------|
| absent | `~/repo` |
| `"src"` | `~/repo/src` |
| `"~/other"`, `"/abs/path"` | that path |

Omitting `cwd` therefore gives the common case — panes open where the `zellij`
command was typed — with no configuration at all. A tab `cwd` overrides the
template `cwd`; both follow the table above.

`~` is expanded to `$HOME` at compile time, because Zellij does not expand it
inside KDL.

## Built-in default

Zellaude always compiles one built-in template named `zellaude`, whether or not
the user has configured templates of their own:

| Tab | Panes |
|-----|-------|
| `git` | `lazygit` \| `btop`, side by side |
| `claude` | `claude` |
| `editor` | `nvim` |
| `shell` | plain shell |

`zellij -s work -n zellaude` works on a fresh install with an empty config file.

A user template named `zellaude` replaces the built-in entirely; the two are
never merged. Because the built-in is always present it is never treated as an
orphan, so it cannot be deleted by removing it from the config — renaming it
means defining a template that takes its name. Panes whose command is not
installed show Zellij's own command-failed pane, which can be re-run in place —
the template still opens.

The name is `zellaude` rather than `default` because `~/.config/zellij/layouts/
default.kdl` is Zellij's own and is protected by the ownership rules below,
which would leave a `default` built-in silently inert.

## Compilation

A new module, `src/session_templates.rs`, owns parsing, validation and KDL
generation. It mirrors `custom_layouts.rs` in shape and reuses its command-grid
validation limits.

Generated layouts carry the bar in `default_tab_template`, so tabs the user
opens later in that session also get one:

```kdl
// zellaude-generated v0.5.9 work
layout {
    default_tab_template {
        pane size=1 borderless=true {
            plugin location="file:~/.config/zellij/plugins/zellaude.wasm"
        }
        children
    }
    tab name="git" {
        pane split_direction="vertical" {
            pane command="sh" { args "-lc" "lazygit" }
            pane command="sh" { args "-lc" "btop" }
        }
    }
    tab name="claude" { ... }
    tab name="editor" cwd="/home/user/repo/src" { ... }
    tab name="shell"
}
```

The plugin's own location comes from its pane in the pane manifest, as
`CustomLayout::to_kdl` already does. Compilation runs once per plugin instance,
after `load_config` succeeds and that location is known.

## File ownership

`name` becomes a filename, which makes it the only dangerous input here.

**Name validation** — `[A-Za-z0-9._-]`, 1–64 characters, not `.` or `..`, no
leading `-` (which `zellij -n` would read as a flag). Custom-state IDs allow
arbitrary Unicode; they never become filenames, so the rules differ.

**Ownership marker** — every generated file starts with
`// zellaude-generated <version> <name>`.

| Existing file | Action |
|---------------|--------|
| none | create |
| has the marker | overwrite |
| **no marker** | **leave untouched**, report the conflict on the bar |
| marker, but no longer in the config | delete |

The third row is what keeps a hand-written `default.kdl` — or any other layout
the user owns — from being destroyed by a name collision.

**Atomic writes** — `mktemp` in the target directory then `mv`, the pattern
`installer.rs` already uses. Every tab runs its own plugin instance, so
concurrent compilation is normal rather than exceptional. A file whose content
already matches is not rewritten.

## Errors

Invalid configuration is reported through the existing
`custom_layout_config_error` path, which already renders on the bar. A malformed
`session_templates` value rejects the whole key rather than skipping the bad
entry, matching how `parse_config_document` treats custom states. Rejecting the
key leaves any previously generated files in place; they are not deleted on a
parse failure, so a typo cannot strand a session template mid-day.

## Testing

`tests/session_templates.rs`:

- Parsing — valid and invalid schemas, duplicate `name`, empty `tabs`, empty
  `commands`
- Name validation — `../evil`, `.`, `..`, `-x`, empty, Unicode
- KDL generation — snapshots covering the three tab shapes, the three `cwd`
  forms, `focus`, and grid dimensions
- `~` expansion
- Built-in default — used when the key is absent, replaced when a user template
  shares its name
- The materialize script, executed through real `sh` in a temporary directory
  the way `run_save_config_script` tests already do: unmarked files survive,
  marked files are overwritten, orphans are deleted, symlinks are resolved
  before writing, and a second run is a no-op

## Documentation

- A README "Session templates" section, following the existing "Custom states"
  section, with the `zellij -s work -n zellaude` invocation
- `~/.config/zellij/layouts/<name>.kdl` added to the README's "What it touches"
  table, noting that only marked files are written or removed

## Out of scope

- `zellij -s work --custom work` syntax, which would require a shell wrapper
- Compiling from `install.sh` or a standalone script; the plugin is the only
  compiler
- Tab-level splits beyond a rectangular command grid
- Referencing a `custom_states` ID from a tab
