# Third-Party Notices

LumiControl includes and depends on third-party software. Those components are not relicensed under the PolyForm Noncommercial License and remain available under their respective license terms.

The authoritative dependency inventories are:

- `Cargo.lock` for Rust dependencies
- `apps/lumi-ui/package-lock.json` for JavaScript dependencies
- the ESP32 Arduino core and ArduinoJson manifests used to build the firmware

The current dependency metadata includes MIT, Apache-2.0, BSD, ISC, 0BSD,
BSL-1.0, MPL-2.0, Unicode-3.0, Zlib, CC0-1.0, CC-BY-4.0, OFL-1.1,
Ubuntu-font-1.0, CDLA-Permissive-2.0, and Unlicense terms. Where a dependency
offers a choice that includes MIT or Apache-2.0, LumiControl uses that permissive
option. In particular, the Windows build does not select the optional GPL or
LGPL alternatives exposed by dual-licensed dependency metadata.

Copyright and license notices embedded by upstream projects must be retained in
source and binary distributions. MPL-2.0 files remain under MPL-2.0; no modified
third-party MPL source files are stored in this repository.

Before distributing a binary release, regenerate and review the dependency inventory, preserve all required notices, and bundle any full license texts required by upstream licenses. The repository's dependency audit checks security advisories; it does not replace license-compliance review.
