---
tm-cli: patch
---

`tm import --from trekker` decodes the HTML entities an Agent wrote into a task's title or description (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&#39;`, `&#x27;`), so `Build &amp; Run` arrives as `Build & Run`; any other `&` is kept. `tm create` and `tm edit` warn when a title or description they are given holds one of those entities, and store it as written.
