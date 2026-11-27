import Foundation

/// A demo thread whose answer is written as agents write on the Mac (`t-markdown`): the recap under
/// its rule, a loose numbered list that starts above 1 with a nested bullet and a code block inside
/// an item, a task list, a quote over several lines, a four-backtick fence holding a three-backtick
/// one, a picture straight after text, a link to another thread, and a notice with bold marks.
extension DemoMac {
    static func markdownThread(endingAt now: Int64) -> [(ItemBody, Int64?, ItemExtras?)] {
        let m: Int64 = 60_000
        return [
            (.user(text: "Finish the session cache: make it survive a restart, then tell me what's left.", images: 0), now - 12 * m, nil),
            (.notice("**Compacted** the conversation so far: `96K` tokens freed."), now - 11 * m, nil),
            (.assistant(text: markdownAnswer, streaming: false), nil, nil),
            (.turnEnd(tookSecs: 318), now - 6 * m, nil),
        ]
    }

    /// The answer of `t-markdown`.
    static let markdownAnswer = """
    ---
    **Done**
    The session cache survives a restart: it's written to `cache/sessions.bin` on shutdown and read back on boot.

    **Still to do**
    Nothing.

    **Found**
    #128 is merged, so the old `SESSION_NO_CACHE` flag can go.

    ## What changed

    Steps 1 and 2 were done in [Cache the session lookups](trek://thread/t-parity). The rest:

    3. Write the cache on shutdown.

       - only entries read in the last hour
       - at most `10_000` of them
         - oldest dropped first

    4. Read it back on boot, before the first request:

       ```rust
       let cache = SessionCache::load(&path).unwrap_or_default();
       ```

    5. Drop an entry whose rotation counter moved.

    Left for the release:

    - [x] Tests for a rotated token
    - [x] A cold-start benchmark
    - [ ] A note in `CHANGELOG.md`

    > A cold cache after a deploy sent every request to Postgres at once.
    > p99 spiked for about a minute.
    > Now the first requests after a boot are served from the file.

    How the docs show the reload command:

    ````markdown
    Reload the cache with:

    ```sh
    trek cache reload
    ```
    ````

    The settings page after the change:
    ![Settings, after](docs/screens/settings-after.png)
    """
}
