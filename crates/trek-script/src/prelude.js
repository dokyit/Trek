// The program's whole world: the host's tools as async functions, print, and timers. The natives
// come off globalThis first, so a program reaches them only through these wrappers, and the
// wrappers keep their own JSON and Object.hasOwn, so a program that replaces or pollutes those
// breaks only itself.
(() => {
  const call = globalThis.__trek_call;
  const out = globalThis.__trek_print;
  const nap = globalThis.__trek_sleep;
  const wake = globalThis.__trek_wake;
  const end = globalThis.__trek_end;
  for (const k of ["__trek_call", "__trek_print", "__trek_sleep", "__trek_wake", "__trek_end"]) {
    delete globalThis[k];
  }
  // No shared memory: Atomics.wait would block inside one instruction, where neither the wall
  // clock nor a Stop can reach it.
  for (const k of ["SharedArrayBuffer", "Atomics", "WebAssembly"]) delete globalThis[k];

  const { parse, stringify } = JSON;
  const own = Object.hasOwn;

  // Strings print as they are, errors with their stack, everything else as compact JSON.
  const show = (v) => {
    if (typeof v === "string") return v;
    if (v instanceof Error) {
      const head = `${v.name}: ${v.message}`;
      return v.stack ? `${head}\n${v.stack}`.trimEnd() : head;
    }
    if (v === undefined || v === null || typeof v !== "object") return String(v);
    try {
      return stringify(v) ?? String(v);
    } catch {
      return String(v);
    }
  };
  const print = (...a) => {
    out(a.map(show).join(" "));
  };
  const ms = (n) => Math.max(0, Number(n) || 0);
  const tool = async (name, args) => {
    const json = stringify(args ?? {});
    if (json === undefined) throw new TypeError(`${name}: arguments must be a JSON object`);
    const r = parse(await call(String(name), json));
    if (own(r, "e")) throw new Error(`${name}: ${r.e}`);
    return r.r;
  };

  // Timers: each has an id the native knows, so clearTimeout ends its wait at once instead of
  // leaving it running after the program returns.
  let nextId = 1;
  const setTimeout = (fn, n, ...a) => {
    const id = nextId++;
    nap(ms(n), id).then((fired) => {
      if (fired && typeof fn === "function") fn(...a);
    });
    return id;
  };
  const clearTimeout = (id) => {
    wake(Number(id) || 0);
  };

  Object.assign(globalThis, {
    print,
    tool,
    console: { log: print, info: print, warn: print, error: print, debug: print },
    sleep: (n) => nap(ms(n), nextId++).then(() => undefined),
    setTimeout,
    clearTimeout,
    bash: (command, o = {}) => tool("bash", { command: String(command), timeout_s: o.timeout }),
    read: async (path, o = {}) => {
      const r = await tool("read", { path: String(path), start_line: o.start, end_line: o.end });
      return r !== null && typeof r === "object" && own(r, "content") ? r.content : r;
    },
    write: (path, content) => tool("write", { path: String(path), content: String(content) }),
    edit: (path, old, neu) => tool("edit", { path: String(path), old: String(old), new: String(neu) }),
  });

  // Starts the program (the body of an async function) and reports its end: a returned value
  // is printed, an uncaught error goes to `end` as text. Gone before the program's first line.
  globalThis.__trek_run = (main) => {
    delete globalThis.__trek_run;
    return main().then(
      (v) => {
        if (v !== undefined) print(v);
        end();
      },
      (e) => end(e instanceof Error ? show(e) : `Uncaught ${show(e)}`),
    );
  };
})();
