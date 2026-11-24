## Control arm `none-docs` against arm `none` (#183)

Descriptive: **nothing is tested**; intervals are 95 % bootstrap over the tasks, one session per task and set. Claude Code 2.1.331, claude-sonnet-5-5 in all. `none-docs` has one more sentence in the prompt: "Also update existing documentation and tests that the change makes wrong or incomplete." *Recorded*: the 20 sessions of `none` (finished 2026-11-24T10:50 to 2026-11-24T12:18 UTC); *concurrent*: 9 more on the tasks with docs units, interleaved with `none-docs` (2026-11-24T14:34 to 2026-11-24T16:39; `none-docs` 2026-11-24T14:29 to 2026-11-24T16:49). Recorded against concurrent shows drift and session noise; what is left between them and the sentence cannot be told apart.

**Undecided:** the touched and the covered units (undecided ones counted both ways), or the two bases, disagree, a base has no docs gap, or an interval contains 50 %. Marc decides.

| Share of the docs gap closed | Recorded `none` as base | Interval | Concurrent `none` as base |
|---|---:|---|---:|
| touched | 0.54 | [0.12, 0.88] | 0.43 |
| covered, undecided ones not | 0.56 | [0.19, 0.89] | 0.43 |
| covered, undecided ones too | 0.56 | [0.19, 0.89] | 0.43 |

Sensitivity without `retry-max-attempts-to-max-retries` (a pause of the run lies between its two sessions, which count above): touched 0.54 (recorded), 0.43 (concurrent); covered, undecided ones not 0.56 (recorded), 0.43 (concurrent); covered, undecided ones too 0.56 (recorded), 0.43 (concurrent); by the same rule: undecided.

More than 0.50 is "most". Forced tests' gap closed (decides nothing): touched 0.94, covered_strict 0.31, covered_lenient 0.31.

| | `none` recorded (20) | `none` concurrent (9) | `none-docs` (20) | `none-docs` minus `none` |
|---|---:|---:|---:|---|
| Mean recall (a) | 0.661 | 0.547 | 0.874 | recorded +0.212 [+0.062, +0.388]; concurrent, 9 tasks +0.194 [+0.000, +0.417] |
| docs: units touched | 2/13 | 3/13 | 8/13 | |
| remote: units touched | 44/45 | 11/11 | 44/45 | |
| forced tests: units touched | 11/14 | 9/9 | 13/14 | |
| docs: units covered (undecided: not - too) | 1/13 | 3/13 | 8/13 | |
| forced tests: units covered (undecided: not - too) | 11/14 | 9/9 | 12/14 | |
| tasks with docs: all / any touched | 2 / 2 of 9 | 3 / 3 of 9 | 4 / 6 of 9 | |
| Recall with new tests | 0.454 | 0.464 | 0.661 | |
| Solved | 20 | 9 | 20 | |
| Tokens, total | 5,743,243 | 3,414,091 | 8,078,987 | recorded 1.41x [1.14x, 1.72x]; concurrent, 9 tasks 1.26x [1.06x, 1.49x] |
| Tokens on the tasks with docs | 3,377,254 | 3,414,091 | 4,287,324 | |
| Cost, total (USD) | 4.47 | 2.57 | 5.58 | |
| Wall time, mean s (without stalls) | 172 (172) | 172 (172) | 213 (213) | |
| Files changed | 106 | 48 | 132 | |
| Over-editing: docs files outside the commit (sessions with any) | 0 (0) | 0 (0) | 1 (1) | |
| Over-editing: tests files outside the commit (sessions with any) | 0 (0) | 0 (0) | 5 (5) | |
| Over-editing: code files outside the commit (sessions with any) | 2 (1) | 1 (1) | 3 (3) | |
| In the commit but not needed: new tests / optional touched | 3 / 0 | 1 / 0 | 10 / 2 | |
| Calls refused by the hook / by Claude Code (with `cd`) | 36 / 4 (4) | 22 / 0 (0) | 39 / 5 (4) | |

| Task | (a) recorded / concurrent / `none-docs` | Docs | Forced tests | Remote | Found now | Missed by both | Missed now only | Differs between the two `none` | Tokens | Extra files (docs/tests/code) |
|---|---|---|---|---|---|---|---|---|---|---|
| `api-version-deprecation-handler-handler-arg` | 1.00 / – / 1.00 | 0/0 / – / 0/0 | 2/2 / – / 2/2 | 2/2 / – / 2/2 | – | – | – | – | 188,991 / – / 210,036 | 0/0/0 / – / 0/0/0 |
| `bean-provider-parameterized-type-reference` | 1.00 / – / 1.00 | 0/0 / – / 0/0 | 0/0 / – / 0/0 | 5/5 / – / 5/5 | – | – | – | – | 157,647 / – / 326,730 | 0/0/0 / – / 0/0/0 |
| `beans-max-nested-path-depth` | 0.00 / 0.00 / 0.00 | 0/1 / 0/1 / 0/1 | 0/0 / 0/0 / 0/0 | 0/0 / 0/0 / 0/0 | – | `data-binding.adoc` | – | – | 400,085 / 453,353 / 726,191 | 0/0/0 / 0/0/0 / 0/1/0 |
| `client-builders-deep-copy-default-headers-cookies` | 1.00 / – / 1.00 | 0/0 / – / 0/0 | 0/0 / – / 0/0 | 2/2 / – / 2/2 | – | – | – | – | 278,983 / – / 682,695 | 0/0/0 / – / 0/0/0 |
| `content-negotiation-no-fallback-mapping` | 1.00 / – / 1.00 | 0/0 / – / 0/0 | 0/0 / – / 0/0 | 1/1 / – / 1/1 | – | – | – | – | 211,427 / – / 372,872 | 0/0/0 / – / 0/1/0 |
| `deferred-bean-registrar-contains-bean` | 1.00 / – / 1.00 | 0/0 / – / 0/0 | 0/0 / – / 0/0 | 3/3 / – / 3/3 | – | – | – | – | 335,208 / – / 598,876 | 0/0/2 / – / 1/0/1 |
| `error-response-interceptor-with-request` | 1.00 / – / 1.00 | 0/0 / – / 0/0 | 0/0 / – / 0/0 | 6/6 / – / 6/6 | – | – | – | – | 271,660 / – / 414,756 | 0/0/0 / – / 0/1/0 |
| `form-converter-map-payloads` | 0.80 / 0.80 / 0.80 | 0/1 / 0/1 / 0/1 | 1/1 / 1/1 / 1/1 | 3/3 / 3/3 / 3/3 | – | `message-converters.adoc` | – | – | 397,621 / 403,403 / 544,248 | 0/0/0 / 0/0/0 / 0/0/0 |
| `http-range-honor-if-range` | 1.00 / – / 1.00 | 0/0 / – / 0/0 | 0/0 / – / 0/0 | 4/4 / – / 4/4 | – | – | – | – | 254,098 / – / 315,006 | 0/0/0 / – / 0/0/0 |
| `jdbcclient-batch-entry-dsl` | 1.00 / 1.00 / 1.00 | 1/1 / 1/1 / 1/1 | 3/3 / 3/3 / 3/3 | 0/0 / 0/0 / 0/0 | – | – | – | – | 1,177,556 / 805,114 / 877,731 | 0/0/0 / 0/0/0 / 0/0/0 |
| `jsonl-media-type-support` | 0.80 / – / 0.80 | 0/0 / – / 0/0 | 0/1 / – / 0/1 | 8/9 / – / 8/9 | – | `ProtobufJsonEncoder.java`, `HttpMessageWriterViewTests.java` | – | – | 195,616 / – / 254,659 | 0/0/0 / – / 0/1/1 |
| `message-source-default-charset` | 0.00 / – / 1.00 | 0/0 / – / 0/0 | 0/1 / – / 1/1 | 0/0 / – / 0/0 | `ResourceBundleMessageSourceTests.java` | – | – | – | 137,074 / – / 253,025 | 0/0/0 / – / 0/0/0 |
| `mime-type-parameter-names-case-insensitive` | 1.00 / – / 1.00 | 0/0 / – / 0/0 | 0/0 / – / 0/0 | 1/1 / – / 1/1 | – | – | – | – | 170,231 / – / 131,066 | 0/0/0 / – / 0/0/0 |
| `observation-record-handled-exception` | 0.62 / 0.62 / 0.88 | 0/3 / 0/3 / 2/3 | 0/0 / 0/0 / 0/0 | 5/5 / 5/5 / 5/5 | `UserController.java`, `UserController.java` | `observability.adoc` | – | – | 222,257 / 236,886 / 467,760 | 0/0/0 / 0/0/1 / 0/1/0 |
| `retry-max-attempts-to-max-retries` | 1.00 / 1.00 / 1.00 | 1/1 / 1/1 / 1/1 | 5/5 / 5/5 / 5/5 | 2/2 / 2/2 / 2/2 | – | – | – | – | 581,447 / 664,243 / 875,007 | 0/0/0 / 0/0/0 / 0/0/0 |
| `retryable-method-retry-event` | 0.50 / – / 1.00 | 0/0 / – / 0/0 | 0/1 / – / 1/1 | 1/1 / – / 1/1 | `RetryTemplateTests.java` | – | – | – | 165,054 / – / 231,942 | 0/0/0 / – / 0/0/0 |
| `servlet-response-ignore-body-flush` | 0.50 / 0.50 / 0.50 | 0/1 / 0/1 / 0/1 | 0/0 / 0/0 / 0/0 | 1/1 / 1/1 / 1/1 | – | `appendix.adoc` | – | – | 133,798 / 193,646 / 212,956 | 0/0/0 / 0/0/0 / 0/0/1 |
| `spel-simple-context-restricted-conversion` | 0.00 / 0.00 / 0.50 | 0/2 / 0/2 / 1/2 | 0/0 / 0/0 / 0/0 | 0/0 / 0/0 / 0/0 | `evaluation.adoc` | `appendix.adoc` | – | – | 357,077 / 458,845 / 354,680 | 0/0/0 / 0/0/0 / 0/0/0 |
| `test-context-cache-pause-mode` | 0.00 / 0.00 / 1.00 | 0/2 / 0/2 / 2/2 | 0/0 / 0/0 / 0/0 | 0/0 / 0/0 / 0/0 | `appendix.adoc`, `caching.adoc` | – | – | – | 70,401 / 121,579 / 126,370 | 0/0/0 / 0/0/0 / 0/0/0 |
| `type-mismatch-required-type-arg` | 0.00 / 1.00 / 1.00 | 0/1 / 1/1 / 1/1 | 0/0 / 0/0 / 0/0 | 0/0 / 0/0 / 0/0 | `mvc-ann-rest-exceptions.adoc` | – | – | `mvc-ann-rest-exceptions.adoc` | 37,012 / 77,022 / 102,381 | 0/0/0 / 0/0/0 / 0/0/0 |

