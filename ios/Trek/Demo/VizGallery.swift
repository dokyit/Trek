import Foundation

// MARK: Native visualizations

extension DemoMac {
    /// One answer of each visualization type, as the Mac's mock agent writes them
    /// (`trek_agents::mock::VIZ_GALLERY`), by its `type`.
    static let vizGallery: [(kind: String, answer: String)] = [
        ("bar", #"""
        Build time is dominated by the app crate; the rest are small.

        ```trek-viz
        {"version":1,"title":"Build time by crate","summary":"Clean release build, seconds per crate.","type":"bar","bars":[{"label":"trek-app","value":142},{"label":"trek-core","value":48},{"label":"trek-agents","value":31},{"label":"gpui-kit","value":96,"tone":"info"},{"label":"vendor/gpui-base","value":22,"tone":"neutral"}],"x_label":"seconds"}
        ```
        """#),
        ("line", #"""
        Latency fell after Tuesday's deploy everywhere but the workers.

        ```trek-viz
        {"version":1,"title":"p95 latency by service","summary":"Daily p95; the workers spike on Thursday.","type":"line","x_labels":["Mon","Tue","Wed","Thu","Fri","Sat","Sun"],"series":[{"label":"API","values":[212,205,168,171,160,158,152]},{"label":"Web","values":[248,240,201,196,190,184,181]},{"label":"Workers","values":[310,302,296,488,371,322,305]}],"area":true,"y_label":"p95","unit":"ms"}
        ```
        """#),
        ("donut", #"""
        Most of the bundle is the editor.

        ```trek-viz
        {"version":1,"title":"Where the bundle's bytes go","summary":"Release binary by component.","type":"donut","parts":[{"label":"Editor & LSP","value":18.4},{"label":"GPUI","value":12.1},{"label":"Tree-sitter grammars","value":9.6},{"label":"Agents","value":4.2},{"label":"Everything else","value":3.1,"tone":"neutral"}],"unit":"MB"}
        ```
        """#),
        ("stats", #"""
        The week in four numbers.

        ```trek-viz
        {"version":1,"title":"This week","summary":"Against last week.","type":"stats","tiles":[{"label":"Threads","value":"184","delta":"+22%","trend":[20,24,22,31,28,30,29],"tone":"positive"},{"label":"Tokens","value":"12.9M","delta":"+8%","trend":[1.6,1.9,1.7,2.1,1.8,2.0,1.8]},{"label":"Spend","value":"$41.20","delta":"-6%","tone":"positive"},{"label":"Failed turns","value":"3","delta":"+2","trend":[0,0,1,0,0,2,0],"tone":"negative"}]}
        ```
        """#),
        ("table", #"""
        Three services need attention; workers most of all.

        ```trek-viz
        {"version":1,"title":"Service health","summary":"Last 24 hours.","type":"table","columns":["Service","Requests","p95 (ms)","Errors","Owner"],"rows":[["API",1284000,152,0.02,"Platform"],["Web",842300,181,0.04,"Frontend"],{"cells":["Workers",96400,488,1.8,"Data"],"tone":"warning"},["Database",3120000,12,0,"Platform"],{"cells":["Search",44100,920,4.1,"Discovery"],"tone":"negative"}]}
        ```
        """#),
        ("heatmap", #"""
        Incidents cluster on Thursday.

        ```trek-viz
        {"version":1,"title":"Incidents by service and weekday","summary":"The worker queue peaks on Thursday.","type":"heatmap","x_labels":["Mon","Tue","Wed","Thu","Fri"],"y_labels":["API","Web","Workers","Database"],"values":[[1,0,1,1,0],[0,1,0,1,1],[1,2,2,7,3],[0,0,1,1,0]]}
        ```
        """#),
        ("treemap", #"""
        Where the code lives.

        ```trek-viz
        {"version":1,"title":"Lines of Rust by module","summary":"trek-app, by top-level module.","type":"treemap","items":[{"label":"workspace","weight":6400},{"label":"thread_view","weight":3900},{"label":"ide","weight":3600},{"label":"root","weight":2100},{"label":"settings_view","weight":1900},{"label":"remote","weight":1500},{"label":"composer","weight":1400},{"label":"panels","weight":1300},{"label":"sidebar","weight":900},{"label":"visualization","weight":1700,"tone":"info"},{"label":"md","weight":600},{"label":"shots","weight":850}]}
        ```
        """#),
        ("timeline", #"""
        The plan fits the quarter with two weeks to spare.

        ```trek-viz
        {"version":1,"title":"Q4 plan","summary":"Phases by team; GA lands in the first week of January.","type":"timeline","ticks":["Oct","Nov","Dec","Jan"],"events":[{"label":"Design review","start":"Oct","end":"Oct","lane":"Design","tone":"info"},{"label":"Prototype","start":"Oct","end":"Nov","lane":"Engineering"},{"label":"Hardening","start":"Dec","end":"Dec","lane":"Engineering","tone":"warning"},{"label":"Beta","start":"Nov","end":"Dec","lane":"Release","tone":"positive"},{"label":"GA","start":"Jan","lane":"Release","tone":"positive"}]}
        ```
        """#),
        ("flow", #"""
        A message takes this path from the composer to the transcript.

        ```trek-viz
        {"version":1,"title":"How a turn flows","summary":"From the composer to the agent and back, with the parts that can stop it.","type":"flow","nodes":[{"id":"composer","label":"Composer","detail":"text, images, mode","group":"App"},{"id":"workspace","label":"Workspace","detail":"routes the turn","group":"App","tone":"accent"},{"id":"session","label":"Agent session","detail":"ACP or direct","group":"Agents"},{"id":"permission","label":"Permission card","detail":"Supervised only","group":"App","tone":"warning"},{"id":"tools","label":"Tools","detail":"edits, commands","group":"Agents"},{"id":"transcript","label":"Transcript","detail":"streamed answer","group":"App","tone":"positive"}],"edges":[{"from":"composer","to":"workspace","label":"send"},{"from":"workspace","to":"session"},{"from":"session","to":"permission","label":"asks"},{"from":"session","to":"tools"},{"from":"permission","to":"tools","label":"allowed"},{"from":"tools","to":"transcript"},{"from":"session","to":"transcript","label":"answer"}]}
        ```
        """#),
        ("layers", #"""
        Trek's stack, top to bottom.

        ```trek-viz
        {"version":1,"title":"Trek's architecture","summary":"Each layer only calls the one beneath it.","type":"layers","layers":[{"label":"Views","detail":"GPUI windows, panels and the transcript","items":["Sidebar","Transcript","IDE","Settings"],"tone":"accent"},{"label":"Workspace","detail":"threads, routing, settings, the remote","items":["Router","Store","Remote"],"tone":"info"},{"label":"Agents","detail":"ACP and direct sessions behind one trait","items":["Claude","Codex","Mock"],"tone":"positive"},{"label":"Core","detail":"storage, transcripts, schemas","items":["SQLite","Paths","trek-viz"],"tone":"neutral"}]}
        ```
        """#),
        ("mockup", #"""
        Two ways to show a release in the browser.

        ```trek-viz
        {"version":1,"title":"Release dashboard","summary":"A browser view with a nav, a search field and the rollout status.","type":"mockup","nodes":[{"kind":"frame","device":"browser","text":"releases.internal","children":[{"kind":"nav","text":"Trek Ops","children":[{"kind":"text","text":"Deploys"},{"kind":"text","text":"Incidents"},{"kind":"avatar","text":"Ada Lovelace"}]},{"kind":"row","children":[{"kind":"card","text":"Rollout","children":[{"kind":"progress","text":"Canary to 100%","value":"64","tone":"info"},{"kind":"badge","text":"Healthy","tone":"positive"}]},{"kind":"card","text":"Search","children":[{"kind":"input","text":"Filter services"},{"kind":"toggle","text":"Only failing","value":"off"}]}]},{"kind":"image","text":"Latency chart","value":"wide"},{"kind":"row","children":[{"kind":"button","text":"Promote"},{"kind":"button","text":"Roll back","tone":"negative"}]}]}]}
        ```
        """#),
    ]

    /// A block that closes but can't be drawn: `color` isn't a field of a bar chart.
    static let vizInvalid = #"""
        Here are the build times, though this block won't draw:

        ```trek-viz
        {"version":1,"title":"Build time by crate","summary":"Clean release build, seconds per crate.","type":"bar","bars":[{"label":"trek-app","value":142},{"label":"trek-core","value":48}],"color":"ember"}
        ```

        The app crate is still the slow one.
        """#

    /// An answer whose chart the demo streams in, a few characters at a time, as an agent writes it.
    static let vizStreaming = #"""
        Test time follows build time, except for the agents' integration suite.

        ```trek-viz
        {"version":1,"title":"Test time by crate","summary":"Full test run on CI, seconds per crate.","type":"bar","bars":[{"label":"trek-app","value":214},{"label":"trek-agents","value":168,"tone":"warning"},{"label":"trek-core","value":71},{"label":"trek-remote","value":38},{"label":"gpui-kit","value":26,"tone":"info"}],"x_label":"seconds"}
        ```

        The agents' suite spends most of its time waiting on mock sessions, so that's the one to look at.
        """#

    /// `-TrekVizKind bar`: the visualizations thread shows only that type (or `invalid`, or `stream`).
    static var vizKind: String? { UserDefaults.standard.string(forKey: "TrekVizKind") }

    /// `-TrekVizHold 120`: the streamed chart stops after this many characters of its JSON, still
    /// being written (for screenshots of a chart part-way).
    static var vizHold: Int? {
        UserDefaults.standard.object(forKey: "TrekVizHold").map { _ in UserDefaults.standard.integer(forKey: "TrekVizHold") }
    }

    /// The visualizations thread: one answer with every type, one that won't draw, then one being
    /// written (`streamed`: what's there before the stream starts, and the rest).
    static func vizThread() -> (gallery: String?, invalid: String?, streamed: (seed: String, rest: String)?) {
        let kind = vizKind
        let gallery = vizGallery.filter { kind == nil || $0.kind == kind }.map(\.answer)
        let fence = vizStreaming.range(of: "```trek-viz\n")!.upperBound
        return (gallery.isEmpty ? nil : (kind == nil ? "Here's one of each, so you can see how they draw.\n\n" : "") + gallery.joined(separator: "\n\n"),
                kind == nil || kind == "invalid" ? vizInvalid : nil,
                kind == nil || kind == "stream" ? (String(vizStreaming[..<fence]), String(vizStreaming[fence...])) : nil)
    }

    /// Text split the way a model streams it: short runs, ending at word breaks (`mock::tokens`).
    static func tokens(_ text: String) -> [String] {
        var out: [String] = []
        var run = ""
        for c in text {
            run.append(c)
            if (c.isWhitespace && run.count >= 3) || run.count >= 12 {
                out.append(run)
                run = ""
            }
        }
        if !run.isEmpty { out.append(run) }
        return out
    }
}
