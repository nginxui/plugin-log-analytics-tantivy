# Changelog

All notable changes to this plugin are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[semantic versioning](https://semver.org/).

## [0.1.0-beta.1]

### Added

- First beta. It serves the pages and routes of `com.nginxui.log-analytics`:
  the log list state, structured search, entries, the traffic dashboard, the
  country, province and city maps, the statistics of a match, the preflight
  check, the rebuild and warm up routes, the map files, the IP location database
  download and the `/events` stream.
- The search box needs every word of the query, has no stop words, matches the
  start of numbers with dots, decoded and encoded text alike, CJK text and IPv6
  addresses.
- Content tracking of the log files: rotations, copy and truncate, compressed
  copies, partial last lines, rewritten files and restarts do not lose or
  repeat lines. A stopped first import resumes.
- The plugin conflicts with `com.nginxui.log-analytics`.
- Error logs are indexed beside the access logs and searched in the
  structured view by level, client, request path and text, with `level:` in
  the search box. They have no dashboard.
