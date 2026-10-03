//! Smoke test: detect agents and index local threads into a throwaway database.
fn main() {
    let t = std::time::Instant::now();
    let agents = trek_core::runtime().block_on(trek_core::detect::detect_all());
    for a in &agents {
        println!("{:<16} {:?} {}", a.name, a.availability, a.version.clone().unwrap_or_default());
    }
    println!("detect: {:?}", t.elapsed());
    let t = std::time::Instant::now();
    let store = trek_core::store::Store::in_memory().unwrap();
    let s = trek_core::import::import_all(&store, &Default::default());
    println!("import: {} Claude Code, {} Codex, {} OpenCode in {:?}", s.claude_code, s.codex, s.opencode, t.elapsed());
    let left_out: Vec<String> = s.skipped.iter().map(|(rule, n)| format!("{n} {}", rule.label())).collect();
    println!("left out: {}", if left_out.is_empty() { "none".into() } else { left_out.join(", ") });
    println!("projects: {}", store.projects().unwrap().len());
    for th in store.threads().unwrap().iter().take(5) {
        println!("  [{}] {} — {}", th.source.label(), th.title, th.cwd.as_ref().map(|p| trek_core::paths::tildify(p)).unwrap_or_default());
    }
    let first = store.threads().unwrap().into_iter().find(|t| t.source == trek_core::ThreadSource::ClaudeCode).unwrap();
    let t = std::time::Instant::now();
    let items = trek_core::import::load_transcript(first.source, first.native_id.as_deref().unwrap()).unwrap();
    println!("claude transcript: {} items in {:?}", items.len(), t.elapsed());
    for src in [trek_core::ThreadSource::Codex, trek_core::ThreadSource::OpenCode] {
        if let Some(th) = store.threads().unwrap().into_iter().find(|t| t.source == src) {
            let items = trek_core::import::load_transcript(src, th.native_id.as_deref().unwrap()).unwrap();
            println!("{} transcript: {} items", src.label(), items.len());
        }
    }
}
