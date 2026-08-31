# Structure API errors

Public callers branch on stable error classes rather than parsing prose. Detailed diagnostics may add context without changing the class.

| Class | Meaning | Typical recovery |
| --- | --- | --- |
| `invalid_request` | Required fields are missing or incompatible | Correct the structured request |
| `invalid_pattern` | A syntax pattern does not parse for the selected language | Use one valid syntax node or wrap required context |
| `unsupported` | Language, mode, relationship, or ref kind is not supported | Choose a supported mode or narrow the claim |
| `not_found` | Requested symbol, snapshot, or ref does not exist | Re-orient or recover from the minting store |
| `ambiguous` | A name resolves to several candidates | Supply path, language, or symbol identity |
| `coverage_incomplete` | The requested claim exceeds indexed or parsed coverage | Repair coverage before claiming absence |
| `stale` | Evidence belongs to an older repository snapshot | Retry with freshness repair |
| `cold_index_pending` | Required index work is still publishing | Retry after the reported delay |
| `budget_exceeded` | Complete evidence cannot fit the requested bounds | Narrow scope or follow continuation evidence |
| `cancelled` | The owning frame cancelled the request | Retry in a fresh frame when safe |
| `busy` | A bounded store or publication lease is unavailable | Retry after the reported backoff |
| `corrupt` | Stored evidence fails schema or digest verification | Preserve the store and rebuild from source authority |
| `internal` | GraphZero violated an internal invariant | Retain diagnostics and report a defect |

An empty hit list is a successful result only when the request was valid. Its absence classification and coverage determine what the caller may conclude.
