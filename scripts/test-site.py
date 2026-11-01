#!/usr/bin/env python3
"""Check static-site metadata, structured data, sitemap, local links and command coverage."""

import json
import re
from html.parser import HTMLParser
from pathlib import Path
from urllib.parse import unquote, urlsplit
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
ORIGIN = "https://sessionsnap.pawanpaudel.com.np"
REPO = "https://github.com/pawanpaudel93/sessionsnap"
INSTALL = f"curl -fsSL {ORIGIN}/install.sh | sh"
RAW_INSTALLER = "https://raw.githubusercontent.com/pawanpaudel93/sessionsnap/main/install.sh"
PAGES = {"index.html": "/", "docs.html": "/docs"}


class Page(HTMLParser):
    def __init__(self, text):
        super().__init__(convert_charrefs=True)
        self.meta = {}
        self.canonicals = []
        self.links = []
        self.ids = []
        self.titles = []
        self.headings = []
        self.schemas = []
        self.capture = None
        self.buffer = []
        self.feed(text)
        self.close()

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        if "id" in attrs:
            self.ids.append(attrs["id"])
        if tag == "meta" and "media" not in attrs:
            key = attrs.get("name", attrs.get("property"))
            self.meta.setdefault(key, []).append(attrs.get("content", ""))
        if tag == "link" and attrs.get("rel") == "canonical":
            self.canonicals.append(attrs.get("href"))
        if tag in ("a", "link") and "href" in attrs:
            self.links.append(attrs["href"])
        if tag in ("img", "script") and "src" in attrs:
            self.links.append(attrs["src"])
        if tag in ("title", "h1") or (tag == "script" and attrs.get("type") == "application/ld+json"):
            self.capture = tag
            self.buffer = []

    def handle_data(self, data):
        if self.capture:
            self.buffer.append(data)

    def handle_endtag(self, tag):
        if tag != self.capture:
            return
        text = "".join(self.buffer).strip()
        if tag == "title":
            self.titles.append(text)
        elif tag == "h1":
            self.headings.append(text)
        elif tag == "script":
            self.schemas.append(json.loads(text))
        self.capture = None


def kebab(name):
    return re.sub(r"(?<!^)([A-Z])", r"-\1", name).lower()


def cli_commands():
    """Subcommand names from the clap `Command` enum (variants are kebab-cased by clap)."""
    source = (ROOT / "src" / "interfaces" / "cli" / "mod.rs").read_text()
    body = source.split("pub enum Command {", 1)[1].split("\n}\n", 1)[0]
    return [kebab(m) for m in re.findall(r"^    ([A-Z][A-Za-z]*)\b", body, re.M)]


def check():
    texts = {name: (SITE / name).read_text() for name in PAGES}
    pages = {name: Page(text) for name, text in texts.items()}
    errors = []

    def require(condition, message):
        if not condition:
            errors.append(message)

    version = re.search(r'^version = "([^"]+)"', (ROOT / "Cargo.toml").read_text(), re.M).group(1)

    titles, descriptions = [], []
    for name, page in pages.items():
        canonical = ORIGIN + PAGES[name]
        require(len(page.titles) == 1 and bool(page.titles[0]), f"{name}: need one title")
        require(len(page.headings) == 1 and bool(page.headings[0]), f"{name}: need one H1")
        require(page.canonicals == [canonical], f"{name}: canonical must be {canonical}")
        require(len(page.ids) == len(set(page.ids)), f"{name}: duplicate element IDs")
        require('<html lang="en">' in texts[name], f"{name}: html lang")
        require('id="main"' in texts[name] and 'href="#main"' in texts[name], f"{name}: skip link to main")
        require(f">v{version}</span>" in texts[name], f"{name}: nav must show v{version} (Cargo.toml)")
        description = page.meta.get("description", [])
        require(len(description) == 1 and bool(description[0]), f"{name}: need one description")
        if description:
            descriptions.append(description[0])
        if page.titles:
            titles.append(page.titles[0])
        for key, expected in {
            "og:url": canonical,
            "og:title": page.titles[0] if page.titles else "",
            "og:description": description[0] if description else "",
            "og:image": ORIGIN + "/og.png",
            "twitter:card": "summary_large_image",
            "twitter:title": page.titles[0] if page.titles else "",
            "twitter:description": description[0] if description else "",
            "twitter:image": ORIGIN + "/og.png",
        }.items():
            require(page.meta.get(key) == [expected], f"{name}: missing or inconsistent {key}")
        require(len(page.schemas) == 1, f"{name}: need one JSON-LD block")
        if page.schemas:
            schema = page.schemas[0]
            require(schema.get("@context") == "https://schema.org", f"{name}: schema context")
            require("aggregateRating" not in schema and "review" not in schema, f"{name}: unsupported ratings")
            if name == "index.html":
                require(schema.get("@type") == "SoftwareSourceCode", f"{name}: software schema type")
                require(schema.get("url") == canonical, f"{name}: schema URL")
                require(schema.get("codeRepository") == REPO, f"{name}: repository")
                require(schema.get("license") == "https://opensource.org/license/mit", f"{name}: license")
                require(schema.get("operatingSystem") == "macOS, Linux", f"{name}: operatingSystem (cmux on macOS, tmux on macOS and Linux)")
                require(schema.get("version") == version, f"{name}: schema version must be {version}")
            else:
                require(schema.get("@type") == "BreadcrumbList", f"{name}: breadcrumb schema type")
                items = schema.get("itemListElement", [])
                require([i.get("position") for i in items] == [1, 2], f"{name}: breadcrumb positions")
                require([i.get("item") for i in items] == [ORIGIN + "/", canonical], f"{name}: breadcrumb URLs")

        for link in page.links:
            parsed = urlsplit(link)
            if parsed.scheme or parsed.netloc:
                if parsed.netloc != urlsplit(ORIGIN).netloc:
                    continue
            route = unquote(parsed.path)
            require(not route.endswith(".html"), f"{name}: noncanonical link {link}")
            if not route:
                target = SITE / name
            else:
                target = SITE / route.lstrip("/")
                if route == "/":
                    target = SITE / "index.html"
                elif not target.suffix:
                    target = target.with_suffix(".html")
            # The installer is served by a hosting redirect rather than a local file.
            if route == "/install.sh":
                continue
            require(target.is_file(), f"{name}: broken local link {link}")
            if parsed.fragment and target.name in pages:
                require(unquote(parsed.fragment) in pages[target.name].ids, f"{name}: missing fragment {link}")

    require(len(set(titles)) == len(PAGES), "page titles must be unique")
    require(len(set(descriptions)) == len(PAGES), "page descriptions must be unique")
    for asset in ("og.png", "apple-touch-icon.png", "favicon.svg", "styles.css", "script.js"):
        require((SITE / asset).is_file(), f"missing site/{asset}")
    require((ROOT / "src/interfaces/web/assets/favicon.svg").read_bytes() == (ROOT / "assets" / "logo.svg").read_bytes(),
            "src/interfaces/web/assets/favicon.svg (the web UI's icon) must be identical to assets/logo.svg")
    require((SITE / "favicon.svg").read_bytes() == (ROOT / "assets" / "logo.svg").read_bytes(),
            "site/favicon.svg must be identical to assets/logo.svg")

    sitemap = ET.parse(SITE / "sitemap.xml")
    urls = [e.text for e in sitemap.findall(".//{http://www.sitemaps.org/schemas/sitemap/0.9}loc")]
    require(sorted(urls) == sorted(ORIGIN + route for route in PAGES.values()), "sitemap must list every canonical once")
    require(f"Sitemap: {ORIGIN}/sitemap.xml" in (SITE / "robots.txt").read_text(), "robots.txt must advertise sitemap")
    llms = (SITE / "llms.txt").read_text()
    require(".html" not in llms, "llms.txt must use canonical URLs")
    require(INSTALL in llms, "llms.txt must give the install command")

    # The install redirect, the one-liner and the docs' doc-nav must agree with the contract.
    vercel = json.loads((SITE / "vercel.json").read_text())
    require(vercel.get("cleanUrls") is True and vercel.get("trailingSlash") is False, "vercel.json: clean URLs")
    require({"source": "/install.sh", "destination": RAW_INSTALLER, "permanent": False} in vercel.get("redirects", []),
            "vercel.json: /install.sh must redirect temporarily to the raw installer")
    require(f"/install.sh {RAW_INSTALLER} 302" in (SITE / "_redirects").read_text(), "_redirects: install redirect")
    require(f'data-copy="{INSTALL}"' in texts["index.html"], "index.html: copy button must copy the install command")
    require(INSTALL in texts["docs.html"], "docs.html must give the install command")

    # Every subcommand in the source has a row on the docs page, plus the two the release adds.
    docs = texts["docs.html"]
    commands = set(cli_commands()) | {"update", "uninstall"}
    require(len(commands) > 10, "could not read the Command enum from src/interfaces/cli/mod.rs")
    for command in sorted(commands):
        require(f"sessionsnap {command}" in docs, f"docs.html must document `sessionsnap {command}`")
        require(f"`{command}`" in llms, f"llms.txt must list `{command}`")
    for flag in ("--check", "--keep-data", "SESSIONSNAP_VERSION", "SESSIONSNAP_INSTALL_DIR", "SESSIONSNAP_NO_CMUX"):
        require(flag in docs, f"docs.html must mention {flag}")

    # The docs name every socket variable sessionsnap reads and the flags each agent restores.
    readme_text = (ROOT / "README.md").read_text()
    for doc, text in {"README.md": readme_text, "docs.html": docs}.items():
        for var in ("SESSIONSNAP_CMUX_SOCKET", "$CMUX_SOCKET_PATH", "$CMUX_SOCKET`" if doc == "README.md" else "$CMUX_SOCKET<"):
            require(var in text, f"{doc} must mention {var.strip('`<')}")
        flat = " ".join(text.split())
        require("allow-list" in flat, f"{doc} must say flags are saved from an allow-list")
        for word in ("--disallowedTools", "--permission-mode", "model_reasoning_effort", "--ask-for-approval"):
            require(word in flat, f"{doc} must list {word} among the flags restored")
        require("even in prose" not in flat, f"{doc} still describes the old credential-guessing filter")

    # Linux: sessionsnap saves and restores tmux there, but Linux cmux is not supported yet.
    for doc, text in {**texts, "llms.txt": llms}.items():
        flat_text = " ".join(text.split())
        require("cmux on Linux is a different app" in flat_text and "not supported yet" in flat_text,
                f"{doc} must say cmux on Linux is not supported yet")
        require("tmux" in flat_text, f"{doc} must say sessionsnap works with tmux")

    # Agent support claims match the README: Claude Code and Codex tested, the rest untested.
    readme = (ROOT / "README.md").read_text()
    readme_agents = re.findall(r"^\| ([^|]+?) \| (Full \(tested\)|Resume only \(untested\)) \|", readme, re.M)
    index = texts["index.html"]
    require(len(readme_agents) >= 2, "could not read the README's supported-agents table")
    for agent, support in readme_agents:
        row = f"<tr><td>{agent}</td><td>"
        require(row in index, f"index.html must list {agent}")
        if row in index:
            cell = index.split(row, 1)[1].split("</td>", 1)[0]
            require(support in cell, f"index.html: {agent} must be marked {support}")

    if errors:
        for error in errors:
            print(f"FAIL: {error}")
        raise SystemExit(1)
    print(f"PASS: {len(pages)} pages — metadata, schemas, sitemap, local links, fragments and command coverage")


if __name__ == "__main__":
    check()
