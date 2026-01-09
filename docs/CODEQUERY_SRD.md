# CodeQuery Tool
## Software Requirements Document
**Version:** 1.0  
**Date:** 2026-01-08  
**Status:** Draft  
**Baseline code reference:** `forge-source.zip`

---

## 0. Change Log
### 0.1 Initial draft
* Initial requirements for a CodeQuery (semantic code search) tool based on `../tools` CodeQuery module.

---

## 1. Introduction

### 1.1 Purpose
Define requirements for adding a semantic code search tool to Forge that uses OpenAI vector stores to index a local repo and answer natural language queries.

### 1.2 Scope
The CodeQuery tool will:
* Discover indexable source files
* Create or reuse a vector store
* Upload file contents with hash metadata
* Reindex incrementally based on file hashes
* Execute semantic search queries

Out of scope:
* Cross-repo federated search
* Alternative embeddings providers (initial version)
* GUI visualization of search results

### 1.3 Definitions
| Term | Definition |
| --- | --- |
| Vector store | OpenAI-managed storage used for file search |
| Indexing | Uploading local files to a vector store |
| Reindex | Incremental update using content hashes |
| Result | A semantic match from the vector store |

### 1.4 References
| Document | Description |
| --- | --- |
| `docs/PROVIDERS_ARCHITECTURE.md` | Provider integration |
| `docs/TOOL_EXECUTOR_SRD.md` | Tool execution framework |
| `../tools/src/codequery/*` | Reference implementation |
| RFC 2119 / RFC 8174 | Requirement keywords |

### 1.5 Requirement Keywords
The key words **MUST**, **MUST NOT**, **SHALL**, **SHOULD**, **MAY** are as defined in RFC 2119.

---

## 2. Overall Description

### 2.1 Product Perspective
CodeQuery is a networked tool executed via Forge's Tool Executor and uses the OpenAI APIs. It relies on the local filesystem for file discovery and hashing.

### 2.2 Product Functions
| Function | Description |
| --- | --- |
| FR-CQ-REQ | Accept query + indexing options |
| FR-CQ-DISC | Discover indexable files with ignore rules |
| FR-CQ-STORE | Resolve/create vector store |
| FR-CQ-IDX | Incremental reindex via file hashes |
| FR-CQ-SEARCH | Run semantic search and return results |

### 2.3 User Characteristics
* End users describe code questions in natural language.
* Developers manage vector stores and API config.

### 2.4 Constraints
* OpenAI API access is required.
* Model names must satisfy Forge provider validation (OpenAI models must start with `gpt-5`).

---

## 3. Functional Requirements

### 3.1 Tool Interface
**FR-CQ-01:** Tool name MUST be `CodeQuery` with aliases `code_query` and `code-query`.

**FR-CQ-02:** Request schema MUST include:
* `query` (string, required)
* `vector_store_id` (string, optional)
* `vector_store_name` (string, optional)
* `file_paths` (array of strings, optional)
* `concurrent_limit` (integer 1-20, optional, default 5)
* `timeout_ms` (integer, optional, default 60000)
* `model` (string, optional)
* `max_num_results` (integer, optional)
* `include_results` (boolean, optional, default false)

**FR-CQ-03:** Response payload MUST include:
* `status` ("ok" | "error")
* `vector_store_id`
* `vector_store_name`
* `indexed_count`
* `skipped_count`
* `deleted_count`
* `query`
* `answer` (assistant-generated text)
* Optional `results` when `include_results=true`

### 3.2 File Discovery
**FR-CQ-04:** When `file_paths` is omitted, CodeQuery MUST auto-discover files from the current working directory using `.gitignore` rules.

**FR-CQ-05:** Discovery MUST skip common non-code directories (e.g., `.git`, `node_modules`, `target`) and exclude binary/media/archive files.

**FR-CQ-06:** Markdown and documentation files SHOULD be excluded by default.

### 3.3 Vector Store Lifecycle
**FR-CQ-07:** If `vector_store_id` is provided, it MUST be used directly.

**FR-CQ-08:** If `vector_store_name` is provided, the tool MUST resolve it via local cache or remote lookup, and create it if missing.

**FR-CQ-09:** Store ID mappings MUST be cached on disk for reuse across sessions.

### 3.4 Incremental Reindexing
**FR-CQ-10:** Each uploaded file MUST include `path` and `hash` attributes.

**FR-CQ-11:** Reindexing MUST upload only changed files, delete removed files, and update moved files based on content hash.

**FR-CQ-12:** The tool MUST respect a concurrency limit for upload operations.

### 3.5 Query Execution
**FR-CQ-13:** The tool MUST execute semantic search using OpenAI Responses API or File Search APIs and return a natural language answer.

**FR-CQ-14:** When `include_results=true`, the tool MUST include top-N matches in the response payload.

### 3.6 Errors and Retries
**FR-CQ-15:** Transient errors (timeouts, 429, 5xx) MUST be retried with exponential backoff (max 3 attempts).

**FR-CQ-16:** Validation errors (missing query, empty file set) MUST return without retries.

---

## 4. Non-Functional Requirements

### 4.1 Security
| Requirement | Specification |
| --- | --- |
| NFR-CQ-SEC-01 | API keys MUST be read from Forge config or environment |
| NFR-CQ-SEC-02 | File uploads MUST exclude non-indexable/binary files |

### 4.2 Performance
| Requirement | Specification |
| --- | --- |
| NFR-CQ-PERF-01 | Hashing MUST be linear in file size |
| NFR-CQ-PERF-02 | Indexing SHOULD avoid re-uploading unchanged files |

### 4.3 Reliability
| Requirement | Specification |
| --- | --- |
| NFR-CQ-REL-01 | Partial indexing failures MUST surface in response metadata |
| NFR-CQ-REL-02 | Indexing timeouts MUST return a clear error |

---

## 5. Configuration

```toml
[codequery]
enabled = false
cache_path = "${HOME}/.forge/codequery/stores.json"
default_vector_store_name = ""
default_concurrent_limit = 5
default_timeout_ms = 60000
include_results = false
```

```toml
[codequery.discovery]
respect_gitignore = true
skip_dirs = [".git", "node_modules", "target", "dist", "build"]
exclude_extensions = [".md", ".png", ".jpg", ".zip", ".exe"]
```

```toml
[codequery.retry]
max_attempts = 3
backoff_ms = [200, 500, 1000]
jitter_ms = 50
```

---

## 6. Verification Requirements

### 6.1 Unit Tests
| Test ID | Description |
| --- | --- |
| T-CQ-DISC-01 | Discovery respects .gitignore and skip dirs |
| T-CQ-HASH-01 | Hash change triggers upload |
| T-CQ-HASH-02 | Unchanged file is skipped |
| T-CQ-STORE-01 | Cache lookup resolves store ID |
| T-CQ-RETRY-01 | 429 triggers retry sequence |

### 6.2 Integration Tests
| Test ID | Description |
| --- | --- |
| IT-CQ-E2E-01 | Index + search returns answer |
| IT-CQ-INC-01 | Delete file removes entry from store |
| IT-CQ-TMO-01 | Timeout yields error payload |

