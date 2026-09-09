# psd Changelog

Types of changes:

- `[added]` for new features.
- `[changed]` for changes in existing functionality.
- `[deprecated]` for once-stable features removed in upcoming releases.
- `[removed]` for deprecated features removed in this release.
- `[fixed]` for any bug fixes.
- `[security]` to invite users to upgrade in case of vulnerabilities.

## Not Yet Published

_Here we list notable things that have been merged into the master branch but have not been released yet._

- [added] Public information on the position of each layer (E.g. `layer_top`, `layer_bottom`. `layer_left`, `layer_right`).
- [added] Support for creating PSD files - `PsdBuilder`, `GroupBuilder` and `LayerBuilder`, including nested layer groups.
- [fixed] A group's opacity, visibility, blend mode and masks are now read from the layer record that opens the folder instead of the hidden bounding section record, which always carries placeholder values.
- [fixed] Parsing a layer whose name fills the record's Pascal string no longer overflows.

## 0.1.8 - April 23, 2020

- [fixed] Parsing of slices resource section [PR][17]

## 0.1.7 - April 11, 2020

- [added] Support for PSD groups [PR][13]
  - @tdakkota

[13]: https://github.com/chinedufn/psd/pull/13
[17]: https://github.com/chinedufn/psd/pull/17
