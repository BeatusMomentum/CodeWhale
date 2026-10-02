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
dock, transcript, pending input,
ocean and character remain in the Engine until their keyboard, mouse,
localization and layout contracts can move together.
