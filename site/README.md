# sessionsnap Site

Static site for `https://sessionsnap.pawanpaudel.com.np`.

Deploy the `site/` directory as the web root. The site is plain HTML/CSS/JS and
does not require a build step.

Pages:

- `index.html` — product page and installer.
- `docs.html` — install, commands, JSON and exit codes, web UI, files, security,
  updating and uninstalling.

Shared assets:

- `styles.css` — site styling, dark and light (follows `prefers-color-scheme`).
- `script.js` — copy buttons and the docs-page section highlight.
- `favicon.svg` — the sessionsnap mark (a copy of `assets/logo.svg`; keep them identical).
- `apple-touch-icon.png`, `og.png` — the mark as a 180px icon and a 1200×630 social card,
  rendered with `rsvg-convert`:

  ```sh
  rsvg-convert -w 180 -h 180 -b '#152238' assets/logo.svg -o site/apple-touch-icon.png
  rsvg-convert assets/og.svg -o site/og.png
  ```

The version shown in the nav (`v0.1.0`) and in the home page's JSON-LD is bumped
by `scripts/release.sh`.

## Search and Canonical URLs

Canonical URLs are `/` and `/docs`, without a trailing slash. HTML canonical
links, Open Graph URLs, the sitemap, `llms.txt` and internal navigation all use
these routes. The physical files remain `index.html` and `docs.html`.

Vercel's `cleanUrls: true` serves these routes and permanently redirects the
`.html` URLs; `trailingSlash: false` keeps the docs URL consistent. If moving to
another host, configure equivalent extensionless routes and redirects before
deploying. A plain file server does not provide that routing by itself.

The homepage uses `SoftwareSourceCode` JSON-LD for the public repository, MIT
license and operating systems. The docs page uses `BreadcrumbList`. These are
factual descriptions, not a promise of search rich results. Do not add ratings,
reviews or features that are not present in the visible content.

Run the source checks with:

```sh
python3 scripts/test-site.py
```

`scripts/check.sh` also runs them in CI. They check unique metadata, canonical
and social URL agreement, JSON-LD, sitemap coverage, local assets, fragment
links, and that the favicon matches the logo. They do not measure rankings,
index coverage or browser performance.

After deploying:

- Verify `/`, `/docs`, `/robots.txt` and `/sitemap.xml` return HTTP 200 without
  redirects or an `X-Robots-Tag: noindex` header.
- Verify `/index.html` redirects to `/` and `/docs.html` to `/docs`.
- Verify `/install.sh` returns a temporary redirect to the raw installer.
- Inspect the rendered canonical and structured data with Google Search
  Console's URL Inspection tool; submit `/sitemap.xml` there.

## Install Redirect

`/install.sh` should return a temporary redirect to:

```text
https://raw.githubusercontent.com/pawanpaudel93/sessionsnap/main/install.sh
```

Included configs:

- `_redirects` for Netlify and Cloudflare Pages.
- `vercel.json` for Vercel.

The raw URL only works once the repository is public; until then the install
command fails with a 404.

The public install command is:

```sh
curl -fsSL https://sessionsnap.pawanpaudel.com.np/install.sh | sh
```
