# Log Analytics

An [NGINX UI](https://github.com/0xJacky/nginx-ui) plugin for looking into your
nginx access logs. It indexes the logs in the background, lets you search them
with structured filters, and shows how the traffic behaves: page views and
visitors over time, top pages, browsers, systems, devices and where the
visitors come from, on a map.

* Plugin id: `com.nginxui.log-analytics-rs`
* Requires NGINX UI 2.7.0 or newer
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
| `geo_map_path` | text | empty | Folder with the map boundary files (`100000_full.json` and the province files). A relative path is looked up in the plugin data directory, empty means its `maps` folder. A file that is not there is fetched by the page from a public map source. |

## Search

The search box takes words and numbers. A line matches when it has **all** of
them, in any order, and there are no stop words: `how to`, `about` and `the`
find what they say. Numbers with dots match by their start, so `192.168`
finds `192.168.1.10` and `Chrome/126` finds every `126.x` build. Encoded text
matches its decoded form (`%E4%B8%AD` and `中`, `union%20select` and
`union select`), Chinese, Japanese and Korean text matches by character and
pair of characters, and IPv6 addresses match in every spelling.

The path, user agent and referer filters match their words in order.

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
| Linux | amd64, arm64 (static binaries) |
| macOS | amd64, arm64 |
| Windows | amd64, arm64 |

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
cargo run --bin manifest                     # regenerate plugin.json
(cd webapp && bun install && bun run build)  # the browser bundle
./build.sh                                   # the packages in dist/
```

`webapp/` is a copy of the web pages of `com.nginxui.log-analytics`. Only the
plugin id in `webapp/build.constants.ts` differs, and the runtime and asset keys
follow it.
