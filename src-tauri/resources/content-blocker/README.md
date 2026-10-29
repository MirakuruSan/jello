# Filter data shipped with Jello

These files let Jello's built-in ad blocker filter pages from the first launch, before
it has downloaded anything. Jello replaces them with fresh copies from the same sources
in the background and keeps those up to date.

They are separate data files distributed alongside Jello, each under its own license.
They are not part of Jello's source code and are not covered by Jello's Apache-2.0
license. Every file is stored exactly as its publisher distributes it.
`scripts/update-filter-snapshot.ps1` refreshes them; `snapshot.json` records when and
from where.

| Path | Source | License |
| --- | --- | --- |
| `lists/ublock-*.txt` | uBlock Origin filter lists, https://github.com/uBlockOrigin/uAssets | GPL-3.0 |
| `lists/easylist.txt` | EasyList, https://easylist.to/ | GPL-3.0 or CC BY-SA 3.0, distributed here under GPL-3.0 |
| `lists/easyprivacy.txt` | EasyPrivacy, https://easylist.to/ | GPL-3.0 or CC BY-SA 3.0, distributed here under GPL-3.0 |
| `ublock-origin/` | Scriptlet and redirect-resource files from the uBlock Origin release named in `snapshot.json`, https://github.com/gorhill/uBlock | GPL-3.0 |

`LICENSE-GPL-3.0.txt` is the full text of the GNU General Public License, version 3.
The source of every file is the file itself; the projects above publish their full
repositories at the addresses listed.

Peter Lowe's ad server list, which Jello also uses, is not included because it is
published without a redistribution license. Jello downloads it directly from
https://pgl.yoyo.org/adservers/.
