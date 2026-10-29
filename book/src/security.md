# Security & Auditing

`srz` treats untrusted web, file, and network inputs with zero-trust defensive hardening.

---

## 1. 100% Safe Rust

The entire codebase enforces `#![forbid(unsafe_code)]`. No `unsafe` blocks exist anywhere in the application code, preventing memory corruption vulnerabilities, buffer overflows, and use-after-free conditions.

---

## 2. Server-Side Request Forgery (SSRF) Defense

When ingesting external URLs:
* **Private IP Blocking**: Ingestion clients reject requests resolving to loopback (`127.0.0.1`), private RFC 1918 subnets (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`), and link-local addresses (`169.254.0.0/16`).
* **YouTube Authority Pinning**: YouTube caption downloads strictly validate the hostname against `*.youtube.com` and force the `https://` scheme.

---

## 3. Prompt Injection Defense

When injecting untrusted text into LLM prompt templates:
* **Closing Tag Delimiter Escaping**: Untrusted inputs are wrapped inside custom `<untrusted_content>` tags. Any closing tags (`</untrusted_content>`) within the user text are escaped, preventing an attacker from escaping the context block.
* **Metadata Sanitization**: Title, author, and URL metadata headers are scrubbed of newline-based prompt escape sequences before synthesis.

---

## 4. Terminal Escape & BiDi Sanitization

Malicious text rendered in a terminal can exploit ANSI escape sequences or Unicode Bidirectional (BiDi) overrides to hide text, overwrite terminal lines, or spoof prompts.

`srz` passes all terminal outputs through `sanitize_terminal_text`:
* Strips ANSI escape sequences (`\x1b[...]`).
* Strips 8-bit C1 control sequences and string-type controls (`OSC`, `DCS`, `SOS`, `PM`, `APC`).
* Strips Unicode BiDi override characters (`U+202A` through `U+202E`, `U+2066` through `U+2069`).

---

## 5. Resource Bounds & Denial-of-Service Defense

* **Bounded HTTP Stream Buffers**: HTTP readers enforce a maximum 10 MB payload limit and 1 MB SSE line limit to prevent memory exhaustion from infinite streams.
* **Symlink Traversal Prevention**: Cache and file writers reject symlinks and directory traversal attacks (`../`).
* **Cryptographic Attestations**: Binary builds are attested with SLSA Provenance certificates via GitHub Sigstore.
