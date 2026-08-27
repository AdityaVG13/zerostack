# Head-to-head bake-off methodology

This artifact is the honest scaffold for graphzero-chb. It compares GraphZero against stack-graphs, SCIP-consumer, and repomap-class graph tools only when each competitor can run locally under identical conditions. Missing local adapters are reported as unavailable, not as wins.

Axes:

1. Fresh and warm index cost.
2. Call/blast correctness against the committed gold set.
3. Tokens per completed navigation task, including any recovery follow-up needed to regain exact source bytes.
4. Staleness handling after an edit.

Current report status: GraphZero metrics are measured from committed benchmark artifacts; competitor-class rows are placeholders with explicit unavailability reasons. Losses remain published.
