# 11 — @trySynara — Synara iOS performance (Gemini 4 Neon, reviewed by Fable 5.2)
- URL: https://x.com/trySynara/status/2124880834440360282
- Date: 2026-11-23 15:00 UTC (26 likes, 2.2K views)
- Text: "Gemini 4 Neon improved Synara iOS performance. Fable 5.2 reviewed it and remeasured the numbers under load. Before → after, from the benchmarks in the PR" + list: Remote failover across dead routes 92% faster (~8,030 → 651 ms); Local connect handshake at 25 ms latency 33% faster (84 → 57 ms); Huge chat snapshot ack 98.6% (19.8 → 0.27 ms); Huge chat decode 63% (37.3 → 13.8 ms); 1,000-chat list sync 84% (87.3 → 13.8 ms); Streaming 500 deltas 84% (213.8 → 33.5 ms); Local cache 91% smaller (1.94 MB → 172 KB); Saving/loading chats 1.5–2× faster; Thread list updates 98.8% (87.6 → 1 ms); Chat JSON decode 93% (32.7 → 2.3 ms); Activity updates 87% (2.76 → 0.36 ms); 4 MB HTTP body read 99.5% (38.9 → 0.21 ms); 1 MB WebSocket frame masking 98% (2.33 → 0.05 ms); Markdown preprocessing 80% (3 → 0.59 ms). "Same UI, no new features."
- Media: one chart image (seen), "Synara iOS — Same app. Much faster." with the same rows grouped Connection / Sync and streaming / Storage / CPU; footnote "Before → after, from the PR benchmarks. Remeasured under load by Fable 5.2, so real gains are a bit higher. Improved with Gemini 4 Neon. Same UI, no new features." No video.
- Product: Synara iOS app.
- Claim: large microbenchmark speedups on the phone client, produced by one model and verified by another.
- Relation to Trek: Trek's iPhone companion would be compared on reconnect/failover time, streaming throughput, large-chat decode, cache size. Also a marketing pattern: per-PR before/after numbers with a "model X optimized, model Y verified" story.
- Image: <scratch>/media/HX0X25vaQAEj6mb.jpg
