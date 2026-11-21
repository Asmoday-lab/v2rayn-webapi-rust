#!/usr/bin/env python3
"""Deploy the Nova SPA into the existing Termux nginx aggregation site.

Idempotent: safe to re-run after rebuilding nova-dist. nginx.conf is edited
in place with CRLF preserved, and the API's Origin guard is satisfied by
forwarding the original Host header (http://$http_host == browser Origin).
"""
import pathlib
import shutil

HOME = pathlib.Path.home()
NGINX = HOME / ".config/nginx"
HTML = NGINX / "html"
CONF = NGINX / "nginx.conf"
SRC = HOME / "v2rayn-nova/nova-dist"
DST = HTML / "nova"
D = chr(36)  # literal '$', kept out of the source to survive any shell wrapping

# 1. Copy the built SPA into the nginx document root.
if DST.exists():
    shutil.rmtree(DST)
shutil.copytree(SRC, DST)
for item in DST.rglob("*"):
    item.chmod(0o755 if item.is_dir() else 0o644)
print("dist copied ->", DST)

# 2. Pin relative asset URLs so SPA deep-link refreshes resolve to /nova/assets/.
idx = DST / "index.html"
html = idx.read_text(encoding="utf-8")
if '<base href="/nova/">' not in html:
    html = html.replace(
        '<meta charset="UTF-8" />',
        '<meta charset="UTF-8" />\n    <base href="/nova/" />',
        1,
    )
    idx.write_text(html, encoding="utf-8")
print("base href present")

# 3. Add two nginx locations, inserted before the existing navigation site.
raw = CONF.read_bytes().decode("utf-8")
nl = "\r\n" if "\r\n" in raw else "\n"
if "location /nova/" not in raw:
    block = nl.join(
        [
            "        # 1a. v2rayN Nova — same-origin SPA",
            "        location /nova/ {",
            "            root   /data/data/com.termux/files/home/.config/nginx/html;",
            "            index  index.html;",
            "            try_files " + D + "uri " + D + "uri/ /nova/index.html;",
            "        }",
            "",
            "        # 1b. v2rayN WebAPI (Rust) — loopback:5080, same origin via /api/",
            "        location /api/ {",
            "            proxy_pass http://127.0.0.1:5080;",
            "            proxy_http_version 1.1;",
            "            proxy_set_header Host " + D + "http_host;",
            "            proxy_buffering off;",
            "            proxy_read_timeout 3600s;",
            "            proxy_send_timeout 3600s;",
            "        }",
            "",
        ]
    )
    marker = "        # 1. 默认静态网页服务（导航主页）" + nl
    if marker not in raw:
        marker = "        location / {" + nl
    if marker not in raw:
        raise SystemExit("nginx insertion marker not found; edit manually")
    CONF.write_bytes(raw.replace(marker, block + marker, 1).encode("utf-8"))
    print("nginx.conf updated")
else:
    print("nginx.conf already has /nova/ location")

# 4. Add the Nova card to the aggregation page.
portal = HTML / "index.html"
page = portal.read_text(encoding="utf-8")
if "'/nova/'" not in page:
    old = "{name:'WebLight',desc:'卧室灯',path:'/light/'}"
    new = old + ",{name:'v2rayN Nova',desc:'代理面板',path:'/nova/'}"
    if old not in page:
        raise SystemExit("portal card marker not found; edit manually")
    portal.write_text(page.replace(old, new, 1), encoding="utf-8")
    print("portal card added")
else:
    print("portal already has Nova card")
