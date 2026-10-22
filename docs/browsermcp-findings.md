# dock-ybc.15: Remove the browsermcp connector

**Finding:** Nine browsermcp processes running (fpl-desktop-bridge connect
--connector browsermcp), zero calls across 581 transcripts (28 Jul - 8 Sep 2026).

**Blocker:** The browsermcp connector is registered at the system level by
fpl-desktop-bridge, not in this repository. Removal requires either:

1. Running `fpl-desktop-bridge disconnect --connector browsermcp` on each
   machine where it's installed
2. Or updating the Desktop Bridge connector registry (outside this repo's
   boundary) to remove the browsermcp entry

**Status:** Cannot be completed from this repo. Documented the finding and
recorded the integration gap. The connector registration lives in the
fpl-desktop-bridge connector store, not in `scripts/` or `config/`.
