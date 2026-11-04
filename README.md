# Tab and Scoreboard

The tab list, nametags and a sidebar for Play Strawberry servers:

- **Tab list**: each player's name with the rank of their group (text and images), sorted by the
  group's weight, a header and footer, and a score on the right (for example the ping).
- **Nametag** above the head: the group's prefix and suffix, and a score below the name.
- **Sidebar** on the right with up to 15 lines, which each player can turn off.
- Layouts per **world** and per **group**, **placeholders**, values pushed by **other plugins**, and
  each viewer's **language**.

Ranks come from the Play Strawberry permissions plugin (contract `ranks/1`). Without it every
player is in the group `default`. Only changes are sent to players, so an idle server costs
almost nothing.

## Commands and permissions

| Command | What it does | Permission (default) |
| --- | --- | --- |
| `/tab` | Version, players, layouts, whether ranks answer | `tab-and-scoreboard:admin` (op 3) |
| `/tab reload` | Reads the settings and language files again | `tab-and-scoreboard:admin` (op 3) |
| `/sidebar`, `/sidebar on`, `/sidebar off` | Shows or changes your own sidebar; kept across joins | `tab-and-scoreboard:sidebar` (everyone) |

## Settings

`plugins/data/tab-and-scoreboard/config.txt` is written with the built-in settings on the first
start. `/tab reload` reads it again. A file that breaks a rule is refused whole: the console or the
command names the line, and the settings in use stay. A short example:

```text
[settings]
refresh-seconds = 1
sidebar = on
sidebar-default = on
nametags = on
sort = on
language = client
tab-name = "{icon}&6[{prefix}]&r {player}"
nametag-prefix = "{icon}&6[{prefix}]&r "

[group default]
tab-name = {player}
nametag-prefix =

[tab-score]
value = {ping}
format = &7Ping: {value}

[layout default]
worlds = *
groups = *
header = @default.header
footer = @default.footer
title = @default.title
line = @default.line.rank
line = @default.line.world
line = @default.line.online
line = @default.line.ping
```

- `[settings]` also takes `ping-seconds`, `icons` (`text` or `pack-required`), `sidebar-command`,
  `sidebar-on-words`, `sidebar-off-words`, `name-order` (`client` or `exact-case`) and `missing`.
- `[group <name>]` gives a group its own `tab-name`, `nametag-prefix`, `nametag-suffix` and `color`.
- `[layout <name>]` chooses `worlds` and `groups` and holds up to 10 `header`, 10 `footer` and 15
  `line` entries; the first layout that fits the viewer is used. A layout without lines has no
  sidebar.
- `[tab-score]` and `[below-name]` add a number in the tab list or below names.
- `[icon <id>]` defines an image: a font glyph from your server's own resource pack, or a head
  from a skin texture, each with a text `fallback`.
- Colours: `&` codes, `&#RRGGBB`, and gradients `<#RRGGBB>text</#RRGGBB>`. Bedrock players see the
  nearest of the 16 colours and a text-only sidebar.
- Limits: 64 KiB file, 256 characters per text, 32 layouts, 64 groups, 64 images.

### Placeholders

`{player}`, `{ping}`, `{online}`, `{max_players}`, `{world}`, `{group}`, `{prefix}`, `{suffix}`,
`{tps}`, `{icon}`, `{icon:<id>}`, `{head}`, and `{<plugin>:<key>}` for a value another plugin
pushes. Anything else in braces stays as written.

### Your own texts

Built-in texts are `@default.*`. Put your own in `plugins/data/tab-and-scoreboard/lang/<code>.lang`
(`en`, `fr`, `es`, `tr`), one `key=text` per line, and use them as `@my.key`.

### Images from your resource pack

Make a resource pack with a font (private-use characters such as `U+E100`), upload it as your
server's required resource pack in the panel, then use `glyph` and `font` in an `[icon]` section and
set `icons = pack-required`. With `icons = text` (the default) every image shows its fallback text.

## For other plugins

Plugin messages, UTF-8 text with tab-separated fields:

- `placeholders/1`: `set`, `icon` and `clear` values per player or for everyone; shown as
  `{<plugin>:<key>}`.
- `sidebar/1`, `below-name/1`: read or change a player's choice, or hide the sidebar for a reason
  while your plugin needs the screen.
- `teams/1`: ask for collision or nametag visibility for a player instead of making your own team.

Players' choices are stored in `plugins/data/tab-and-scoreboard/players.tsv`.

## Languages

English, French, Spanish and Turkish, chosen by each viewer's game language unless `language` fixes
one (`lang/`).

## Install on a Play Strawberry server

1. Add Tab and Scoreboard to your server in the Play Strawberry panel. Add the Play Strawberry
   permissions plugin too if you want ranks.
2. Restart the server. The plugin creates `plugins/data/tab-and-scoreboard/config.txt`.
3. Edit the file and run `/tab reload`.

Play Strawberry servers load only plugins signed by Play Strawberry, so a file you build yourself
from this source does not load on them.

## Build from source

You need Rust with the `wasm32-wasip2` target and the Strawberry plugin SDK
(`strawberry-plugin-api`), which Play Strawberry gives to its plugin developers. Put the SDK in a
folder named `strawberry-plugin-api` next to this one, or change its path in `Cargo.toml`.

```text
cargo test
cargo build --release --target wasm32-wasip2
```

The plugin file is `target/wasm32-wasip2/release/tab_and_scoreboard.wasm`.

## License

Copyright Strawberry Interactive LLC. The license text is not published yet; until it is, all
rights are reserved.
