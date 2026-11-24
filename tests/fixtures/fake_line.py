#!/usr/bin/env python3
"""Fake LINE Developers Console, LINE Official Account Manager, entry form, login and LINE API.

Used only by loa's end-to-end tests (test-hooks builds). Pages follow the structure the
bundled scenarios expect; they are not copies of the real screens.

    python3 fake_line.py [--port 0] [--auto-login-ms 1200]

Prints `PORT <n>` on stdout once listening. Admin endpoints: GET /__state, POST /__seed (JSON),
POST /__lock?channel=<id>&stage=review.
"""
import html
import json
import random
import sys
import threading
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOCK = threading.Lock()
STATE = {
    "providers": {},  # pid -> {name}
    "channels": {},  # cid -> {pid, name, type, description, email, secret, callbacks, consent, liff: {id: app}, webhook, miniapp: {stage: {liff_id, url, locked}}}
    "oas": {},  # @id -> {name, status_message, join_groups, chat, webhook, auto_reply, members: {name: role}, messaging: {channel_id} | None}
    "tokens": {},  # token -> cid
    "terms_text": "LINE開発者契約 本文 第1版",
    "log": [],
}
AUTO_LOGIN_MS = 1200


def rid(n=10):
    return str(random.randint(10 ** (n - 1), 10 ** n - 1))


def rhex(n=32):
    return "".join(random.choice("0123456789abcdef") for _ in range(n))


def esc(s):
    return html.escape(str(s or ""), quote=True)


TYPE_LABEL = {"login": "LINEログイン", "miniapp": "LINEミニアプリ", "messaging": "Messaging API"}


def page(title, body, header=""):
    return f"""<!doctype html><html lang="ja"><head><meta charset="utf-8"><title>{esc(title)}</title>
<style>body{{font-family:sans-serif;margin:16px}} [hidden]{{display:none!important}} .row{{display:flex;gap:8px;margin:4px 0}}</style>
</head><body>{header}<main>{body}</main>
<script>
function edit(id){{document.getElementById(id).hidden=false;}}
</script></body></html>"""


# Real console (2026-11-24): the account name only exists after the avatar (an <img> with no role)
# is clicked; the menu also holds a logout form next to it.
PROFILE = """<header><div class="profile"><span class="kv-dropdown"><img alt="" src="data:image/gif;base64,R0lGODlhAQABAAAAACw=" width="24" height="24"
onclick="if(!this.nextElementSibling){this.insertAdjacentHTML('afterend','<div class=&quot;content&quot;><div class=&quot;profile-link&quot;><hgroup class=&quot;user-info&quot;><h5 class=&quot;name&quot;>テスト 太郎</h5><h6 class=&quot;email&quot;>taro@example.com</h6></hgroup></div><form method=&quot;post&quot; action=&quot;/dev/logout&quot;><button class=&quot;logout-link&quot;>ログアウト</button></form></div>')}"></span></div></header>"""


def kv(label, value, action=None, field=None, multiline=False, extra=""):
    """Developers console field (2026-11-23): one bare <label>, the value, and 編集 → 更新 / キャンセル
    in the same box. The label is not tied to the input."""
    form, btn = "", ""
    if action:
        fid = "k_" + rhex(6)
        inp = (f'<textarea name="value">{esc(value)}</textarea>' if multiline
               else f'<input name="value" value="{esc(value)}">')
        form = (f'<form id="{fid}" method="post" action="{action}" hidden><input type="hidden" name="field" value="{field}">'
                f'{inp}<button type="submit">更新</button>'
                f'<button type="button" onclick="document.getElementById(\'{fid}\').hidden=true">キャンセル</button></form>')
        btn = f'<button type="button" onclick="edit(\'{fid}\')">編集</button>'
    return (f'<div class="kv-field"><label>{esc(label)} <i></i></label><section><div class="field">'
            f'<span>{esc(value)}</span>{form}</div></section><aside><div class="dc-operations">{btn}{extra}</div></aside></div>')


def group(label, value, form_html=None, extra=""):
    btn = ""
    if form_html is not None:
        fid = "f_" + rhex(6)
        btn = f'<button type="button" onclick="edit(\'{fid}\')">編集</button>'
        form_html = form_html.replace("<form ", f'<form id="{fid}" hidden ', 1)
    return f"""<div role="group" aria-label="{esc(label)}" class="row"><span>{esc(label)}</span><span>{esc(value)}</span>{btn}{extra}{form_html or ''}</div>"""


def edit_form(action, field, label, value, multiline=False, button="更新"):
    iid = "i_" + rhex(6)
    if multiline:
        inp = f'<textarea id="{iid}" name="value">{esc(value)}</textarea>'
    else:
        inp = f'<input id="{iid}" name="value" value="{esc(value)}">'
    return f"""<form method="post" action="{action}"><input type="hidden" name="field" value="{field}"><label for="{iid}">{esc(label)}</label>{inp}<button type="submit">{button}</button></form>"""


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        pass

    # ------------------------------------------------------------ helpers
    def send(self, code, body, ctype="text/html; charset=utf-8", headers=None):
        data = body.encode() if isinstance(body, str) else body
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(data)))
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(data)

    def json(self, code, obj):
        self.send(code, json.dumps(obj, ensure_ascii=False), "application/json")

    def redirect(self, loc, headers=None):
        h = {"Location": loc}
        h.update(headers or {})
        self.send(302, "", headers=h)

    def logged_in(self):
        return "fake_session=1" in (self.headers.get("Cookie") or "")

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(n) if n else b""

    def form(self):
        return {k: v[0] for k, v in urllib.parse.parse_qs(self.body().decode(), keep_blank_values=True).items()}

    def require_login(self):
        if self.logged_in():
            return True
        target = urllib.parse.quote(self.path, safe="")
        self.redirect(f"/acct/login?redirectUri={target}")
        return False

    def bearer_channel(self):
        auth = self.headers.get("Authorization") or ""
        tok = auth.removeprefix("Bearer ").strip()
        return STATE["tokens"].get(tok)

    # ------------------------------------------------------------ routing
    def do_GET(self):
        self.route("GET")

    def do_POST(self):
        self.route("POST")

    def do_PUT(self):
        self.route("PUT")

    def do_DELETE(self):
        self.route("DELETE")

    def route(self, method):
        u = urllib.parse.urlparse(self.path)
        path, q = u.path, {k: v[0] for k, v in urllib.parse.parse_qs(u.query).items()}
        if STATE.get("flags", {}).get("slow") and path.startswith("/dev/"):
            time.sleep(4)
        with LOCK:
            STATE["log"].append(f"{method} {path}")
            try:
                if path.startswith("/api/"):
                    return self.api(method, path[4:], q)
                if path.startswith("/__"):
                    return self.admin(method, path, q)
                if path.startswith("/acct/"):
                    return self.acct(path, q)
                if path.startswith("/evil"):
                    return self.send(200, page("evil", "<h1>outside</h1>"))
                if not self.require_login():
                    return
                if path.startswith("/dev/"):
                    return self.dev(method, path[4:], q)
                if path.startswith("/oam"):
                    return self.oam(method, path[4:] or "/", q)
                if path.startswith("/entry/"):
                    return self.entry(method, path[6:], q)
                self.send(404, page("404", "<h1>見つかりません</h1>"))
            except Exception as e:  # keep the server alive for the rest of the test
                self.send(500, page("500", f"<pre>{esc(repr(e))}</pre>"))

    # ------------------------------------------------------------ login
    def acct(self, path, q):
        if path == "/acct/login":
            to = q.get("redirectUri", "/dev/console/")
            auto = f"<script>setTimeout(()=>location.href='/acct/do-login?redirectUri={urllib.parse.quote(to, safe='')}',{AUTO_LOGIN_MS});</script>" if AUTO_LOGIN_MS >= 0 else ""
            return self.send(200, page("ログイン", f'<h1>LINEビジネスIDでログイン</h1><a href="/acct/do-login?redirectUri={urllib.parse.quote(to, safe="")}">ログイン</a>{auto}'))
        if path == "/acct/do-login":
            return self.redirect(q.get("redirectUri", "/dev/console/"), {"Set-Cookie": "fake_session=1; Path=/"})
        self.send(404, "")

    # ------------------------------------------------------------ admin
    def admin(self, method, path, q):
        if path == "/__state":
            s = {k: v for k, v in STATE.items() if k != "log"}
            return self.json(200, s)
        if path == "/__seed" and method == "POST":
            data = json.loads(self.body() or b"{}")
            for k, v in data.items():
                STATE[k] = v
            return self.json(200, {"ok": True})
        if path == "/__lock" and method == "POST":
            ch = STATE["channels"][q["channel"]]
            ch["miniapp"][q["stage"]]["locked"] = True
            return self.json(200, {"ok": True})
        if path == "/__flag" and method == "POST":
            STATE.setdefault("flags", {})[q["name"]] = q.get("value", "1") == "1"
            return self.json(200, {"ok": True})
        if path == "/__terms" and method == "POST":
            STATE["terms_text"] = self.body().decode()
            return self.json(200, {"ok": True})
        self.send(404, "")

    # ------------------------------------------------------------ developers console
    def dev(self, method, path, q):
        parts = [p for p in path.split("/") if p]
        flags = STATE.get("flags", {})
        if parts == ["console"] and flags.get("evil"):
            return self.redirect(f"http://localhost:{self.server.server_address[1]}/evil")
        # /console/
        if parts == ["console"]:
            create_label = "新規作成する" if flags.get("broken") else "作成"
            items = "".join(f'<li><a href="/dev/console/provider/{pid}">{esc(p["name"])}</a></li>' for pid, p in STATE["providers"].items())
            # Mirrors the real console (2026-11-23): the form is not a dialog, the label is not tied
            # to its textarea, and the buttons sit in a second contentinfo next to the page footer.
            body = f"""<h1>プロバイダー</h1><h2>プロバイダー （{len(STATE["providers"])}）</h2><ul>{items}</ul>
<button type="button" onclick="document.getElementById('np').hidden=false">{create_label}</button>
<div id="np" hidden><form method="post" action="/dev/console/provider/create"><section><h2>新規プロバイダー作成</h2>
<div class="kv-field"><label>プロバイダー名 <i></i></label><section><div class="field"><textarea name="name" placeholder="-"></textarea></div></section></div></section>
<div role="contentinfo"><button type="button" onclick="document.getElementById('np').hidden=true">キャンセル</button><button type="submit">作成</button></div></form></div>
<div role="contentinfo"><a href="/dev/console/">規約とポリシー</a></div>"""
            return self.send(200, page("コンソール", body, PROFILE))
        if parts == ["console", "provider", "create"] and method == "POST":
            f = self.form()
            pid = rid()
            STATE["providers"][pid] = {"name": f["name"]}
            return self.redirect(f"/dev/console/provider/{pid}")
        if len(parts) >= 3 and parts[:2] == ["console", "provider"]:
            pid = parts[2]
            p = STATE["providers"].get(pid)
            if not p:
                return self.send(404, page("404", "<h1>なし</h1>"))
            if len(parts) == 3:
                chans = "".join(
                    f'<li><a href="/dev/console/channel/{cid}/basics"><span class="dc-role-badge">Admin</span><h3 class="title">{esc(c["name"])}</h3><span>{TYPE_LABEL[c["type"]]}</span><div class="dc-status-inner">開発中</div></a></li>'
                    for cid, c in STATE["channels"].items() if c["pid"] == pid)
                body = f'<h1>{esc(p["name"])}</h1><h2>チャネル</h2><ul>{chans}</ul><a href="/dev/console/channel/new?provider={pid}">新規チャネル作成</a> <a href="/dev/console/provider/{pid}/setting">設定</a>'
                return self.send(200, page(p["name"], body))
            if parts[3] == "setting":
                if method == "POST":
                    p["name"] = self.form()["name"]
                    return self.redirect(f"/dev/console/provider/{pid}/setting")
                body = f'<h1>{esc(p["name"])}</h1><form method="post"><label for="pn">プロバイダー名</label><input id="pn" name="name" value="{esc(p["name"])}"><button type="submit">保存</button></form>'
                return self.send(200, page("設定", body))
        if parts == ["console", "channel", "new"]:
            ty = q.get("type", "")
            pid = q.get("provider", "")
            if ty not in ("line-login", "miniapp"):
                return self.send(200, page("新規チャネル", "<h1>チャネルの種類を選択</h1>"))
            # Mirrors the real form (2026-11-23): bare labels next to textareas, a required company
            # country select for LINE Login, and several agreement checkboxes for MINI Apps.
            def field(label, ctrl):
                return f'<div class="kv-field"><label>{label} <i></i></label><section><div class="field">{ctrl}</div></section></div>'
            def agree(i, text):
                return f'<div><input type="checkbox" id="ag{i}" name="ag{i}" required><label for="ag{i}">{text}</label></div>'
            if ty == "line-login":
                extra = (field("会社・事業者の所在国・地域", '<select name="country"><option value="-1" disabled selected>未設定</option><option value="1">日本</option><option value="2">台湾</option></select>')
                         + '<div><input type="checkbox" id="web" name="web"><label for="web">ウェブアプリ</label></div>')
                agrees = agree(0, "LINE開発者契約に同意します。")
            else:
                extra = ""
                agrees = "".join(agree(i, t) for i, t in enumerate([
                    "LINE開発者契約に同意します。",
                    "LINEミニアプリプラットフォーム規約に同意します。",
                    "LINEミニアプリポリシーを遵守します。",
                    "LINEミニアプリを提供する地域と、サービス事業主の所在国・地域が同一であることを表明し、保証します。",
                    "これらの規約に同意すること及びこれらの契約を締結することについて、会社を代表する必要かつ正当な権限を有していることを表明し、保証します。 入力及び操作を行う者が開発受託会社（開発担当企業）の場合は、これらの規約に同意し、これらの規約に基づく契約を締結すること及び以降の必要な操作設定を行うことについてプロバイダーの代理として必要かつ正当な権限を有していることを表明し、保証します。",
                ]))
            body = f"""<h1>新規チャネル作成</h1><form method="post" action="/dev/console/channel/create">
<input type="hidden" name="provider" value="{esc(pid)}"><input type="hidden" name="type" value="{esc(ty)}">
{extra}
{field("チャネル名", '<textarea name="name" placeholder="チャネル名を入力してください" required></textarea>')}
{field("チャネル説明", '<textarea name="description" placeholder="チャネル説明を入力してください"></textarea>')}
{field("メールアドレス", '<textarea name="email" placeholder="メールアドレスを入力してください">owner@example.com</textarea>')}
<div id="terms">{esc(STATE["terms_text"])}</div>
{agrees}
<button type="submit">作成</button></form>"""
            return self.send(200, page("新規チャネル作成", body))
        if parts == ["console", "channel", "create"] and method == "POST":
            f = self.form()
            if f.get("ag0") != "on":
                return self.send(400, page("エラー", "<h1>規約に同意してください</h1>"))
            ty = "login" if f["type"] == "line-login" else "miniapp"
            if ty == "login" and f.get("country", "-1") == "-1":
                return self.send(400, page("エラー", "<h1>所在国・地域を選択してください</h1>"))
            if ty == "miniapp" and any(f.get(f"ag{i}") != "on" for i in range(5)):
                return self.send(400, page("エラー", "<h1>すべてに同意してください</h1>"))
            cid = rid()
            ch = {"pid": f["provider"], "name": f["name"], "type": ty, "description": f.get("description", ""),
                  "email": f.get("email", ""), "secret": rhex(), "callbacks": "", "consent": ty != "login",
                  "liff": {}, "webhook": "", "miniapp": {}}
            if ty == "miniapp":
                for st in ("developing", "review", "published"):
                    ch["miniapp"][st] = {"liff_id": f"{rid()}-{rhex(8)}", "url": "", "locked": False}
            STATE["channels"][cid] = ch
            return self.redirect(f"/dev/console/channel/{cid}/basics")
        if len(parts) >= 4 and parts[:2] == ["console", "channel"]:
            cid, tab = parts[2], parts[3]
            ch = STATE["channels"].get(cid)
            if not ch:
                return self.send(404, page("404", "<h1>チャネルがありません</h1>"))
            base = f"/dev/console/channel/{cid}"
            tabs = f'<nav><a role="tab" href="{base}/basics">チャネル基本設定</a> <a role="tab" href="{base}/line-login">LINEログイン設定</a> <a role="tab" href="{base}/web-app">ウェブアプリ設定</a></nav>'
            head = f'<h1>{esc(ch["name"])}</h1>{tabs}'
            if tab == "consent" and method == "POST":
                ch["consent"] = True
                return self.redirect(f"{base}/basics")
            if tab == "update" and method == "POST":
                f = self.form()
                key = {"name": "name", "description": "description", "email": "email", "callback": "callbacks"}[f["field"]]
                ch[key] = f["value"]
                back = "line-login" if key == "callbacks" else "basics"
                return self.redirect(f"{base}/{back}")
            if tab == "delete" and method == "POST":
                f = self.form()
                if f.get("confirm", "").strip() != cid or f.get("agree") != "on":
                    return self.send(400, page("error", "<p>チャネルIDの入力と同意が必要です</p>"))
                pid = ch["pid"]
                del STATE["channels"][cid]
                return self.redirect(f"/dev/console/provider/{pid}")
            if tab == "basics":
                consent = ""
                if not ch["consent"]:
                    consent = f'<dialog open aria-label="情報利用に関する同意について"><p>情報の利用に同意してください。</p><form method="post" action="{base}/consent"><button type="submit">同意する</button></form></dialog>'
                # Real console (2026-11-24): fields sit in a 基本情報 section, the 多言語対応 table repeats
                # 「チャネル名」 as a column header, and deleting opens a plain box (no role=dialog) that
                # wants the channel ID typed in and a consent checkbox, with its buttons in a contentinfo.
                body = head + consent + "<h2>チャネル基本設定</h2>" + '<section class="tab-section"><h3>基本情報</h3>' + "".join([
                    kv("チャネルID", cid),
                    kv("チャネル名", ch["name"], f"{base}/update", "name", True),
                    kv("チャネル説明", ch["description"], f"{base}/update", "description", True),
                    kv("メールアドレス", ch["email"], f"{base}/update", "email", True),
                    kv("チャネルシークレット", ch["secret"], extra='<button type="button">発行</button>'),
                ]) + f"""</section>
<section class="tab-section"><h3>多言語対応</h3><table><tr><th>言語</th><th>チャネル名</th><th>チャネル説明</th></tr><tr><td colspan="3">データはありません。</td></tr></table></section>
<section class="tab-section"><h3>チャネルの削除</h3><button type="button" onclick="document.getElementById('del').hidden=false">削除</button></section>
<div class="kv-modal" id="del" hidden><form method="post" action="{base}/delete"><header>このチャネルを削除しますか？</header>
<ol><li>一度チャネルを削除すると元に戻せません。</li></ol>
<div><label>確認のためチャネルID（{cid}）を入力してください。</label><textarea name="confirm"></textarea></div>
<div><input type="checkbox" id="delagree" name="agree"><label for="delagree">上記に同意し、このチャネルの永久削除を希望します。</label></div>
<div role="contentinfo"><button type="button" onclick="document.getElementById('del').hidden=true">キャンセル</button><button type="submit">削除</button></div></form></div>
<div role="contentinfo"><a href="/dev/console/">規約とポリシー</a></div>"""
                return self.send(200, page(ch["name"], body))
            if tab == "line-login":
                body = head + kv("コールバックURL", ch["callbacks"], f"{base}/update", "callback", True)
                return self.send(200, page(ch["name"], body))
            if tab == "web-app":
                if method == "POST":
                    f = self.form()
                    st = f["stage"]
                    if ch["miniapp"][st]["locked"]:
                        return self.send(400, page("エラー", "<h1>変更できません</h1>"))
                    ch["miniapp"][st]["url"] = f["value"]
                    return self.redirect(f"{base}/web-app")
                secs = ""
                for st, label in (("developing", "開発用"), ("review", "審査用"), ("published", "本番用")):
                    m = ch["miniapp"].get(st)
                    if not m:
                        continue
                    if m["locked"]:
                        edit = "<p>審査中のため変更できません</p>"
                    else:
                        fid = "w_" + st
                        edit = f'<button type="button" onclick="edit(\'{fid}\')">編集</button><form id="{fid}" hidden method="post"><input type="hidden" name="stage" value="{st}"><label for="u_{st}">エンドポイントURL</label><input id="u_{st}" name="value" value="{esc(m["url"])}"><button type="submit">更新</button></form>'
                    secs += f'<section aria-label="{label}"><h2>{label}</h2><div class="row"><span>LIFF ID</span><span>{esc(m["liff_id"])}</span></div><div class="row"><span>エンドポイントURL</span><span>{esc(m["url"])}</span></div>{edit}</section>'
                return self.send(200, page(ch["name"], head + secs))
        self.send(404, page("404", "<h1>見つかりません</h1>"))

    # ------------------------------------------------------------ official account manager
    def oam(self, method, path, q):
        parts = [urllib.parse.unquote(p) for p in path.split("/") if p]
        if not parts:
            items = "".join(f'<li><a href="/oam/account/{oid}/">{esc(o["name"])}</a></li>' for oid, o in STATE["oas"].items())
            return self.send(200, page("アカウントリスト", f"<h1>アカウントリスト</h1><h2>アカウント({len(STATE['oas'])})</h2><ul>{items}</ul>"))
        if parts[0] != "account" or len(parts) < 2 or parts[1] not in STATE["oas"]:
            return self.send(404, page("404", "<h1>なし</h1>"))
        oid = parts[1]
        o = STATE["oas"][oid]
        header = f'<header><span>{esc(o["name"])}</span> <span>{esc(oid)}</span></header>'
        base = f"/oam/account/{oid}/setting"
        rest = parts[2:]
        if rest == [] :
            return self.send(200, page(o["name"], f"<h1>{esc(o['name'])}</h1>", header))
        if rest == ["setting"]:
            if method == "POST":
                f = self.form()
                if "field" in f:
                    o[f["field"]] = f["value"]
                else:
                    o["join_groups"] = f.get("join_groups") == "on"
                return self.redirect(base)
            jg = "checked" if o["join_groups"] else ""
            body = "".join([
                group("アカウント名", o["name"], edit_form(base, "name", "アカウント名", o["name"], button="保存")),
                group("ステータスメッセージ", o["status_message"], edit_form(base, "status_message", "ステータスメッセージ", o["status_message"], button="保存")),
                group("ベーシックID", oid),
                f'<form method="post" action="{base}"><div role="group" aria-label="機能の利用"><label><input type="checkbox" role="switch" name="join_groups" aria-label="グループ・複数人トークへの参加を許可する" {jg}>グループ・複数人トークへの参加を許可する</label><button type="submit">保存</button></div></form>',
            ])
            return self.send(200, page("アカウント設定", body, header))
        if rest == ["setting", "response"]:
            if method == "POST":
                f = self.form()
                o[f["key"]] = f["value"] == "true"
                return self.json(200, {"ok": True})
            sw = ""
            for key, label in (("chat", "チャット"), ("webhook", "Webhook"), ("auto_reply", "応答メッセージ")):
                c = "true" if o[key] else "false"
                chk = "checked" if o[key] else ""
                sw += f'<div><label><input type="checkbox" role="switch" aria-label="{label}" aria-checked="{c}" {chk} onchange="this.setAttribute(\'aria-checked\', this.checked); fetch(\'{base}/response\', {{method:\'POST\', headers:{{\'Content-Type\':\'application/x-www-form-urlencoded\'}}, body:\'key={key}&value=\'+this.checked}})">{label}</label></div>'
            return self.send(200, page("応答設定", "<h1>応答設定</h1>" + sw, header))
        if rest == ["setting", "messaging-api"]:
            if method == "POST":
                f = self.form()
                prov = f.get("provider") or ""
                if prov == "__new__":
                    pid = rid()
                    STATE["providers"][pid] = {"name": f["new_provider"]}
                else:
                    pid = prov
                cid = rid()
                STATE["channels"][cid] = {"pid": pid, "name": o["name"], "type": "messaging", "description": "", "email": "",
                                          "secret": rhex(), "callbacks": "", "consent": True, "liff": {}, "webhook": "", "miniapp": {}}
                o["messaging"] = {"channel_id": cid}
                return self.redirect(f"{base}/messaging-api")
            if o.get("messaging"):
                ch = STATE["channels"][o["messaging"]["channel_id"]]
                body = "<h1>Messaging API</h1>" + group("Channel ID", o["messaging"]["channel_id"]) + group("Channel secret", ch["secret"])
                return self.send(200, page("Messaging API", body, header))
            radios = "".join(f'<div><input type="radio" name="provider" id="p{pid}" value="{pid}"><label for="p{pid}">{esc(p["name"])}</label></div>' for pid, p in STATE["providers"].items())
            body = f"""<h1>Messaging API</h1><button type="button" onclick="document.getElementById('ma').showModal()">Messaging APIを利用する</button>
<dialog id="ma" aria-label="Messaging APIの利用"><form method="post">{radios}
<div><input type="radio" name="provider" id="pnew" value="__new__"><label for="pnew">プロバイダーを作成</label></div>
<div><label for="np">プロバイダー名</label><input id="np" name="new_provider"></div>
<button type="button" onclick="document.getElementById('okb').hidden=false">同意する</button>
<button type="submit" id="okb" hidden>OK</button></form></dialog>"""
            return self.send(200, page("Messaging API", body, header))
        if rest == ["setting", "user"]:
            if method == "POST":
                f = self.form()
                if f.get("op") == "delete":
                    o["members"].pop(f["member"], None)
                return self.redirect(f"{base}/user")
            rows = ""
            for i, (name, role) in enumerate(o["members"].items()):
                rows += f'<div role="row" aria-label="{esc(name)}"><span>{esc(name)}</span><span>{esc(role)}</span><button type="button" onclick="document.getElementById(\'d{i}\').showModal()">削除</button><dialog id="d{i}" aria-label="メンバーを削除"><form method="post"><input type="hidden" name="op" value="delete"><input type="hidden" name="member" value="{esc(name)}"><button type="submit">削除</button></form></dialog></div>'
            roles = "".join(f'<div><input type="radio" name="role" id="r{i}"><label for="r{i}">{l}</label></div>' for i, l in enumerate(["管理者", "運用担当者", "運用担当者（分析の閲覧権限なし）", "運用担当者（配信権限なし）"]))
            inv = f"https://manager.line.biz/invitation/{rhex(16)}"
            body = f"""<h1>権限管理</h1>{rows}<button type="button" onclick="document.getElementById('add').showModal()">メンバーを追加</button>
<dialog id="add" aria-label="メンバーを追加">{roles}<button type="button" onclick="document.getElementById('inv').hidden=false">URLを発行</button>
<div id="inv" hidden><label for="iu">招待URL</label><input id="iu" readonly value="{inv}"></div></dialog>"""
            return self.send(200, page("権限管理", body, header))
        self.send(404, page("404", "<h1>なし</h1>"))

    # ------------------------------------------------------------ entry form
    def entry(self, method, path, q):
        if path == "/form/entry/unverified":
            if method == "POST":
                f = self.form()
                oid = "@" + rhex(7)
                STATE["oas"][oid] = {"name": f["name"], "status_message": "", "join_groups": True, "chat": True,
                                     "webhook": False, "auto_reply": True, "members": {}, "messaging": None}
                return self.redirect(f"/entry/form/entry/done?id={urllib.parse.quote(oid)}")
            body = """<h1>LINE公式アカウントの作成</h1><form method="post">
<div><label for="an">アカウント名</label><input id="an" name="name"></div>
<div><label for="co">会社・事業者名</label><input id="co" name="company"></div>
<div><label for="c1">大業種</label><select id="c1" name="c1"><option>小売</option><option>飲食</option></select></div>
<div><label for="c2">小業種</label><select id="c2" name="c2"><option>その他</option></select></div>
<button type="button" onclick="document.getElementById('fin').hidden=false">確認</button>
<button type="submit" id="fin" hidden>完了</button></form>"""
            return self.send(200, page("作成", body))
        if path == "/form/entry/done":
            return self.send(200, page("完了", "<h1>作成しました</h1>" + group("ベーシックID", q.get("id", ""))))
        self.send(404, "")

    # ------------------------------------------------------------ LINE API
    def api(self, method, path, q):
        if path == "/oauth2/v3/token" and method == "POST":
            f = self.form()
            ch = STATE["channels"].get(f.get("client_id"))
            if not ch or ch["secret"] != f.get("client_secret") or f.get("grant_type") != "client_credentials":
                return self.json(400, {"error": "invalid_client", "error_description": "invalid client"})
            tok = "eyJhbGciOiJIUzI1NiJ9." + rhex(24) + "." + rhex(24)
            STATE["tokens"][tok] = f["client_id"]
            return self.json(200, {"token_type": "Bearer", "access_token": tok, "expires_in": 900})
        cid = self.bearer_channel()
        if not cid or cid not in STATE["channels"]:
            return self.json(401, {"message": "Authentication failed"})
        ch = STATE["channels"][cid]
        if path == "/liff/v1/apps":
            if ch["type"] != "login":
                return self.json(400, {"message": "LIFF is not available for this channel"})
            if method == "GET":
                if not ch["liff"]:
                    return self.json(404, {"message": "no apps"})
                return self.json(200, {"apps": list(ch["liff"].values())})
            if method == "POST":
                b = json.loads(self.body())
                if len(ch["liff"]) >= 30:
                    return self.json(400, {"message": "max"})
                if not b.get("view", {}).get("url", "").startswith("https://"):
                    return self.json(400, {"message": "invalid url"})
                lid = f"{cid}-{rhex(8)}"
                ch["liff"][lid] = {"liffId": lid, "view": b["view"], "description": b.get("description"),
                                   "permanentLinkPattern": b.get("permanentLinkPattern"), "scope": b.get("scope"), "botPrompt": b.get("botPrompt", "none")}
                return self.json(200, {"liffId": lid})
        if path.startswith("/liff/v1/apps/"):
            lid = urllib.parse.unquote(path.rsplit("/", 1)[1])
            app = ch["liff"].get(lid)
            if not app:
                return self.json(404, {"message": "not found"})
            if method == "PUT":
                b = json.loads(self.body())
                if "view" in b:
                    app["view"].update(b["view"])
                for k in ("description", "botPrompt", "scope"):
                    if k in b:
                        app[k] = b[k]
                return self.json(200, {})
            if method == "DELETE":
                del ch["liff"][lid]
                return self.json(200, {})
        if path == "/v2/bot/channel/webhook/endpoint":
            if ch["type"] != "messaging":
                return self.json(400, {"message": "not a messaging channel"})
            if method == "GET":
                return self.json(200, {"endpoint": ch["webhook"], "active": bool(ch["webhook"])})
            if method == "PUT":
                ch["webhook"] = json.loads(self.body())["endpoint"]
                return self.json(200, {})
        if path == "/v2/bot/channel/webhook/test" and method == "POST":
            return self.json(200, {"success": True, "timestamp": "2026-11-19T00:00:00Z", "statusCode": 200, "reason": "OK", "detail": "200"})
        self.json(404, {"message": "not found"})


def main():
    global AUTO_LOGIN_MS
    port = 0
    args = sys.argv[1:]
    while args:
        a = args.pop(0)
        if a == "--port":
            port = int(args.pop(0))
        elif a == "--auto-login-ms":
            AUTO_LOGIN_MS = int(args.pop(0))
    class Server(ThreadingHTTPServer):
        daemon_threads = True

        def handle_error(self, request, client_address):
            if isinstance(sys.exc_info()[1], (ConnectionResetError, BrokenPipeError)):
                return  # the browser closed a keep-alive connection
            super().handle_error(request, client_address)

    srv = Server(("127.0.0.1", port), H)
    print(f"PORT {srv.server_address[1]}", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
