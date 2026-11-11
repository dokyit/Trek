"""Make a throwaway demo project that is a real, clean git repository, by writing its files
directly: one commit on main, and an index that matches it, so nothing shows as changed.
Only for the isolated test app's frames: a project with a branch.

make_repo.py DIR NAME
"""
import hashlib
import os
import struct
import sys
import time
import zlib

root, name = sys.argv[1], sys.argv[2]
files = {
    "README.md": f"# {name}\n\nA demo project for screenshots.\n",
    "src/cli.rs": "pub struct Config {\n    pub verbose: bool,\n}\n",
    "src/main.rs": 'fn main() {\n    println!("hello");\n}\n',
}
git = os.path.join(root, ".git")
if os.path.exists(git):
    sys.exit(f"{root} already has a repository")
for sub in ("objects", "refs/heads"):
    os.makedirs(os.path.join(git, sub), exist_ok=True)


def put(kind, body):
    data = f"{kind} {len(body)}\0".encode() + body
    sha = hashlib.sha1(data).hexdigest()
    d = os.path.join(git, "objects", sha[:2])
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, sha[2:]), "wb") as f:
        f.write(zlib.compress(data))
    return sha


def tree(entries):  # name -> (mode, sha)
    body = b"".join(f"{mode} {n}".encode() + b"\0" + bytes.fromhex(sha) for n, (mode, sha) in sorted(entries.items()))
    return put("tree", body)


top, src, blobs = {}, {}, {}
for path, text in files.items():
    full = os.path.join(root, path)
    os.makedirs(os.path.dirname(full), exist_ok=True)
    with open(full, "w") as f:
        f.write(text)
    sha = put("blob", text.encode())
    blobs[path] = sha
    (src if path.startswith("src/") else top)[path.split("/")[-1]] = ("100644", sha)
top["src"] = ("40000", tree(src))
when = f"{int(time.time())} +0000"
commit = put("commit", (f"tree {tree(top)}\nauthor Demo <demo@example.com> {when}\ncommitter Demo <demo@example.com> {when}\n\nStart\n").encode())
open(os.path.join(git, "HEAD"), "w").write("ref: refs/heads/main\n")
open(os.path.join(git, "refs/heads/main"), "w").write(commit + "\n")
open(os.path.join(git, "config"), "w").write("[core]\n\trepositoryformatversion = 0\n\tfilemode = true\n\tbare = false\n[user]\n\tname = Demo\n\temail = demo@example.com\n")

# The index (version 2): every file as committed, with the stat git would record for it.
entries = b""
for path in sorted(files):
    st = os.stat(os.path.join(root, path))
    u32 = lambda v: int(v) & 0xFFFFFFFF
    head = struct.pack(">10I", u32(st.st_ctime), 0, u32(st.st_mtime), 0, u32(st.st_dev), u32(st.st_ino),
                       0o100644, u32(st.st_uid), u32(st.st_gid), u32(st.st_size))
    entry = head + bytes.fromhex(blobs[path]) + struct.pack(">H", len(path)) + path.encode()
    entries += entry + b"\0" * (8 - len(entry) % 8)
index = b"DIRC" + struct.pack(">II", 2, len(files)) + entries
open(os.path.join(git, "index"), "wb").write(index + hashlib.sha1(index).digest())
print("repository", root, commit[:7])
