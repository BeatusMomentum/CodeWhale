# Pinned terminal components

Source files in this directory are byte-for-byte from the upstream commit
recorded in UPSTREAM.json. Keep the upstream MIT licence and component
attribution with this source. Preview media and local sessions are excluded.

The Engine consumes the kit through a local path dependency, with no Git
dependency and no package publication. Binary builds need only this checkout.
Publishing codewhale-tui to crates.io requires this kit version on crates.io;
that publication remains a separate release gate requiring founder approval.

Adoption is deliberately incremental: each migrated renderer deletes its
predecessor. Workflow progress, working/verification indicators, the metrics
row and posture row use the kit. MetricsLine and PostureBar own their complete
shed, projection, render and pointer geometry; the Engine supplies measured
facts, localized labels and hints, clock lifecycle, live/custom theme ink and
action dispatch. The current kit drops the pinned right PostureFact's optional
ink during layout. The host carries every fact's exact ink in a distinct role
and uses the shared live-theme adapter, so permission and right-notice colors
remain independent without modifying pinned source. Mounted composer,
dock, transcript, ocean and character remain in the Engine until their keyboard, mouse,
localization and layout contracts can move together.

The Ocean compatibility slice adopts the complete ramp/column pure
sampling facade with explicit live colors, retaining the Engine's monotonic
clocks, life-presence policy and actual-ramp cache identity. Host semantic
finishing still follows the current ColorCompatBackend policy. This is not
yet adoption of the kit's guarded apply/apply_matching path: its known-dark
truecolor-only gate and refusal of unreadable or unknown foregrounds require
explicit reconciliation with current host painting before that move.

PendingCard routes the complete composer pending-input facade through the
kit's measured row plan, shared with its existing generic PendingInputPreview.
Engine-owned queue/context/child-request facts, localized copy, exact palette
styles and terminal backend remain authoritative. The host compositor/wrapping
helpers are deleted; decision settlement remains outside this rendering slice. The existing context action suffixes retain their current English
copy; other native card labels use the Engine localization catalogue.

DecisionBand adopts the complete bottom-anchored ApprovalWidget paint and
geometry through one kit plan: wrapped controls, full/compact validated rule
coverage, persistent-action visibility, body truncation and option-order
mouse rectangles. The host band compositor and save-preview fitting are
deleted. The existing ElevationWidget row measurements also use the kit's
shared Ratatui wrapping helper. Engine request facts, badges, dossier/command
preview formatting, localization, exact palette styles and keyboard/mouse
decisions remain authoritative. Default Enter and timeout still deny; parent
Escape aborts, child Escape hides, and modified/non-press keys cannot grant.
Persistent keys and mouse targets follow the last painted save coverage;
collapsed and empty frames retain canonical empty slots and withdraw stale
geometry. Every styled span and rule-coverage string is display-safe before
measurement and painting. The generic bordered ApprovalCard keeps
its separate verbatim-subject/input contract, and the native band does not
claim its invisible-token encoding. Mounted composer, ElevationWidget painting
and transcript are still outside this slice.
