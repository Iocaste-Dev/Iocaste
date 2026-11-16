# The `ioc` command line (v0.0.7-alpha)

| Command | What it does |
|---|---|
| `ioc new <name>` | Create a project folder (`ioc.toml`, `src/Program.ioc`) |
| `ioc init` | Turn the current folder into a project |
| `ioc check [files] [--syntax-only]` | Syntax, names, types, ownership (includes installed clusters) |
| `ioc build [files] [--emit-c] [--debug]` | Compile to a native program in `./build` |
| `ioc run [files] [-- args]` | Build and run |
| `ioc clean [--locks]` | Delete `./build` (and `ioc.lock` with `--locks`) |
| `ioc install [--toolchain] [--yes]` | Check the C compiler; install clusters from `ioc.toml`; write `ioc.lock` |
| `ioc add <name> [version]` / `ioc add [name] --path <folder>` | Add a cluster |
| `ioc remove <name>` | Remove a cluster |
| `ioc list` | Clusters of this project and their status |
| `ioc doctor` | Health check of the whole setup |
| `ioc version [--verbose]` | Version, platform, C compiler |
| `ioc update [--url u]` | Is a newer ioc available? (exit code 10 = yes) |
| `ioc config [list\|get\|set\|unset]` | Settings: `cc`, `update_url` |
| `ioc env` | Paths and variables in use |
| `ioc ast` / `ioc tokens` | Inspect the front-end output |

## Clusters

A cluster is a folder with its own `ioc.toml` (`name`, `version`, `kind = "cluster"`) and `src/*.ioc`.

```toml
[clusters]
mathx = { path = "../mathx" }                 # works today
mathx = { path = "../mathx", version = "^0.1" }  # also check its version
# "ioc.api" = "^0.1"                          # registry clusters: not online yet
```

`ioc install` resolves them (including the clusters they depend on) and records a content hash in `ioc.lock`.
If a cluster's files change, `ioc build` asks you to run `ioc install` again, so builds never silently use something different.

## Update channel

`ioc update` reads a text file (https URL or local path) set with `ioc config set update_url <where>` or `IOC_UPDATE_URL`.
Line 1: newest version (`0.0.7-alpha`). Line 2 (optional): download link or note. Self-update is manual in the alpha.

## Environment

| Variable | Meaning |
|---|---|
| `IOC_HOME` | Where ioc keeps its config (default `~/.ioc`) |
| `IOC_CC` | C compiler to use (overrides the `cc` setting) |
| `IOC_UPDATE_URL` | Update channel (overrides `update_url`) |
