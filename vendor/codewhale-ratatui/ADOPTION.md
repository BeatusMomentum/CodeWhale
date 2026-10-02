# Pinned terminal components

Source files in this directory are byte-for-byte from the upstream commit
recorded in UPSTREAM.json. Keep the upstream MIT licence and component
attribution with this source. Preview media and local sessions are excluded.

The Engine consumes the kit through a local path dependency, with no Git
dependency and no package publication. Binary builds need only this checkout.
Publishing codewhale-tui to crates.io requires this kit version on crates.io;
that publication remains a separate release gate requiring founder approval.

Adoption is deliberately incremental: each migrated renderer deletes its
predecessor. Workflow progress, working/verification indicators and the
metrics row use the kit. MetricsLine owns its complete shed, projection,
render and pointer geometry; the Engine supplies measured facts, localized
labels and hints, live/custom theme ink and action dispatch. Mounted composer,
dock, posture, transcript, pending input,
ocean and character remain in the Engine until their keyboard, mouse,
localization and layout contracts can move together.
