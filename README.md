# Log Analytics

An [NGINX UI](https://github.com/0xJacky/nginx-ui) plugin for looking into your
nginx access logs. It indexes the logs in the background, lets you search them
with structured filters, and shows how the traffic behaves: page views and
visitors over time, top pages, browsers, systems, devices and where the
visitors come from, on a map.

* Plugin id: `com.nginxui.log-analytics-tantivy`
* Requires NGINX UI 3.0.0 or newer
* Plugin API version 1

The plugin serves the same web pages and the same HTTP routes as
`com.nginxui.log-analytics`, and the two conflict: only one of them can be
installed and enabled at a time. A search box that finds more than the other
plugin did is the one visible difference, see [Search](#search).

## Setting up

Install the plugin from the plugin page and enable it. Indexing starts on its
own: the logs NGINX UI lists are picked up within seconds, and their state shows
in the log list. Disabling the plugin stops the process and frees what it held.

For the visitor map by province and city the plugin needs an IP location
database. Open the plugin settings and download it there. Without it the map
still shows countries.

## Settings

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `incremental_index_interval` | number | 15 | Minutes between two checks of the logs for new lines. Zero or empty means 15. |
| `max_concurrent_index_tasks` | number | 0 | The most log files indexed at the same time. Zero takes one for a small memory budget and at most two otherwise. |
| `index_custom_mmdb` | text | empty | Path of your own IP location database. A relative path is looked up in the `geolite` folder of the plugin data directory. The downloaded database wins when both exist. |

## Search

The search box takes words and numbers. A line matches when it has **all** of
them, in any order, and there are no stop words: `how to`, `about` and `the`
find what they say. Numbers with dots match by their start, so `192.168`
finds `192.168.1.10` and `Chrome/126` finds every `126.x` build. Encoded text
matches its decoded form (`%E4%B8%AD` and `中`, `union%20select` and
`union select`), Chinese, Japanese and Korean text matches by character and
pair of characters, and IPv6 addresses match in every spelling.

The path, user agent and referer filters match their words in order.

Filters go into the same box and combine with AND: `status:404`, `status:5xx`,
`status:400-499`, `method:POST`, `ip:192.168.0.0/16`, `ip:2001:db8::/32`,
`path:/api/`, `ua:curl`, `referer:google`, `browser:`, `os:`, `device:`,
`country:`, `region:`, `city:`, `bytes:>1000`, `rt:>0.5` and `a..b` ranges. A
leading `-` excludes (`-bot`, `-status:404`) and quotes make a phrase. Anything
that does not parse stays plain text, and the response lists such parts in
`query_warnings`.

## Dashboard rollups

The dashboard reads per hour figures (views, bytes, visitors, top lists) that
indexing keeps up to date, and the index only for the hours a window edge or a
day boundary cuts. The rollups are a cache of the index: a rewritten file or a
rebuild drops the rollup of its group, which is computed again from the index,
and after a restart the first request does that once. A group whose rollup
would pass 64 MB is served from the index.

## How the logs are read

A log group is a listed log and its rotated files (`access.log.1`,
`access.log.2.gz`, `access.log-20260101`). A line is known by the first line of
its file and its byte offset, not by the file name. A rotation that renames a
file, a copy that truncates the original and a compressed copy all continue from
where the plugin stopped, and nothing is indexed twice. A line that is still
being written waits for its newline, and a file that was rewritten from its
start replaces its documents.

The read positions are stored with the documents in every commit, so a restart,
or a stop in the middle of a long first import, continues where it left off.
Long imports commit on the way.

## Memory

The indexer sizes itself from the memory the process may use (the control group
limit, otherwise the memory of the machine) and the CPUs it may use:

| Memory | Writer heap | Threads |
| --- | --- | --- |
| under 1 GiB | 50 MB | 1 |
| 1 GiB to 4 GiB | 200 MB | 2 |
| 4 GiB and more | 1 GiB | 4 |

On a dataset of 1.4 million lines the smallest tier stays within 160 MB of
resident memory on Linux. `LOG_ANALYTICS_MEMORY_MB` overrides the detected
budget, which is meant for measuring.

## Permissions and why

| Permission | Why it is needed |
| --- | --- |
| `log.files` | To ask NGINX UI which nginx log files it may read, and to be told when that set changes. The plugin reads those files, and only those and their rotated files, itself. |
| `network` | To download the IP location database from `cloud.nginxui.com`. Nothing else opens a connection. The download goes through the HTTP proxy configured in NGINX UI, when there is one. |

Access logs hold addresses, session parameters and user agents. They stay on the
machine: the index lives in the plugin data directory and no log content is sent
anywhere.

## Supported platforms

| OS | Architectures |
| --- | --- |
| Linux | amd64, arm64, 386, arm, riscv64, loong64 (static binaries) |
| macOS | amd64, arm64 |
| Windows | amd64, arm64, 386 |

These are the platforms Nginx UI is released for, except MIPS and ARMv5: their
Rust targets lack 64-bit atomics. The `linux-arm` package runs on ARMv6 and
ARMv7.

On a 32-bit system the index has to fit the address space of the process, as
it is mapped into memory. A process gets 2 GB on Windows and about 3 GB on
Linux, which holds an index of 1.5 to 2 GB, some 5 to 7 million log lines. Past that an indexing round stops with
an error and the plugin keeps serving what it has.

Each platform ships as its own package, see `build.sh`. On Windows the HTTP API
listens on a loopback port instead of a socket, which the plugin SDK reports to
NGINX UI during the handshake.

## Files

Everything lives in the plugin data directory: `index/` holds the index,
`geolite/` the IP location database and `maps/` the optional map files.

## Development

```
cargo test                                   # unit and integration tests
cargo test --release --test perf_dataset -- --ignored --nocapture
cargo clippy --all-targets -- -D warnings
./build.sh --webapp-only                     # take the webapp into webapp/dist
./build.sh                                   # the packages in dist/
./build.sh --prebuilt DIR                    # package executables built elsewhere
./build.sh --webapp ARCHIVE                  # package another webapp build
```

The tests read the webapp source from a checkout of
`plugin-log-analytics-webapp` next to this repository, or from
`LOG_ANALYTICS_WEBAPP_DIR`.

The SDK comes from crates.io as `nginxui-plugin-sdk`. To build against a local
checkout of `plugin-sdk-rust`, patch it in `.cargo/config.toml`, which git
ignores:

```toml
[patch.crates-io]
nginxui-plugin-sdk = { path = "../plugin-sdk-rust" }
```

## Releases

`plugin.json` is written by hand. After a webapp update, copy the bundle
paths, chunks and `shared` ranges from `webapp/dist/manifest.webapp.json`
into it; the tests compare them.

Pushing a tag `vX.Y.Z` that matches the version in `plugin.json` and
`Cargo.toml` runs `.github/workflows/release.yml`. It builds the executables
on GitHub runners, natively where a runner exists and with cargo zigbuild
otherwise, packages them with
`build.sh --prebuilt`, signs `plugin.sums` with the key of the `release`
environment (`PLUGIN_SIGNING_KEY`, `PLUGIN_SIGNING_KEY_PASSWORD`), verifies the
archives against the `PLUGIN_SIGNING_PUBLIC_KEY` variable and publishes the
archives and their `.sha256` files as a GitHub release. The notes list the features and fixes since the previous tag, generated from
the commit messages by git-cliff (`cliff.toml`).
A version with a prerelease part such as `0.1.0-beta.1` is marked as a
prerelease.

## Webapp

The web pages come from `plugin-log-analytics-webapp`, which both log analytics
plugins share. Its release archive holds one build per plugin id, and
`webapp.lock` names the version of the release this plugin packages, and only
changes after a webapp release.
`build.sh` takes the archive from `--webapp`, from a sibling checkout
(`../plugin-log-analytics-webapp/release`) or from the release download, and
unpacks the build of this plugin into `webapp/dist`; a download is checked
against the `.sha256` file of the release. Features the Go plugin
lacks are announced in the `features` of the preflight answer, such as the
search syntax help.
