// The program's whole world: the host's tools as async functions, print, and sleep. The three
// natives come off globalThis first, so a program reaches them only through these wrappers.
(() => {
  const call = globalThis.__trek_call;
  const out = globalThis.__trek_print;
  const nap = globalThis.__trek_sleep;
  delete globalThis.__trek_call;
  delete globalThis.__trek_print;
  delete globalThis.__trek_sleep;

  // Strings print as they are, errors with their stack, everything else as compact JSON.
  const show = (v) => {
    if (typeof v === "string") return v;
    if (v instanceof Error) {
      const head = `${v.name}: ${v.message}`;
      return v.stack ? `${head}\n${v.stack}`.trimEnd() : head;
    }
    if (v === undefined || typeof v !== "object") return String(v);
    try {
      return JSON.stringify(v) ?? String(v);
    } catch {
      return String(v);
    }
  };
  const print = (...a) => {
    out(a.map(show).join(" "));
  };
  const ms = (n) => Math.max(0, Number(n) || 0);
  const tool = async (name, args) => {
    const r = JSON.parse(await call(String(name), JSON.stringify(args ?? {})));
    if ("e" in r) throw new Error(`${name}: ${r.e}`);
    return r.r;
  };

  Object.assign(globalThis, {
    print,
    tool,
    console: { log: print, info: print, warn: print, error: print, debug: print },
    sleep: (n) => nap(ms(n)),
    setTimeout: (fn, n, ...a) => {
      nap(ms(n)).then(() => fn(...a));
      return 0;
    },
    bash: (command, o = {}) => tool("bash", { command: String(command), timeout_s: o.timeout }),
    read: async (path, o = {}) => {
      const r = await tool("read", { path: String(path), start_line: o.start, end_line: o.end });
      return r && typeof r === "object" && "content" in r ? r.content : r;
    },
    write: (path, content) => tool("write", { path: String(path), content: String(content) }),
    edit: (path, old, neu) => tool("edit", { path: String(path), old: String(old), new: String(neu) }),
  });
})();
