# Open questions and decisions

Questions the spec doesn't answer, with the decision taken (SPEC §0 rule 2).

| # | Question | Decision |
| --- | --- | --- |
| 1 | §1 lists installers as out of scope until Silas decides; Silas asked for installable builds on Windows, macOS and Linux. | Ship per-OS installers (see `docs/INSTALL.md`); unsigned for now. |
| 2 | §13.1 open source or commercial? | Silas (2026-11-14): MIT licence, public repo. OCCT stays dynamically linked (LGPL-2.1 + exception); its notices ship in every package. |
