# Reviewing the compatibility rules

`crates/boothready-core/data/ruleset.json` cites a document for every vendor or community claim. The first review read those documents through web search excerpts, because the cloud environment it ran in couldn't open AlphaTheta's, Denon's or Reddit's sites. A proper review reads the full pages, and it has to run on a machine with open internet.

## 1. Fetch the sources

On your own computer, from the repo:

```sh
cd app
npm ci
npx playwright install chromium
node scripts/fetch-references.mjs
```

It saves every cited page as text (PDF manuals as PDFs) to `research/refs/`, and Reddit threads for the contested claims to `research/reddit/`, with a summary in `research/index.json`. The folder is gitignored because it's copies of other people's pages. A run takes a few minutes, mostly waiting between Reddit requests.

## 2. Review against them

Open a Claude session on the same computer: the Claude Desktop app, or `claude remote-control` in a terminal in the repo. Give it this:

> Read `docs/RULES-REVIEW.md`, then check every claim in `crates/boothready-core/data/ruleset.json` against the pages in `research/`. For each device and aspect, confirm the claim says what the cited page says, fix any claim that doesn't, and upgrade inferred claims where a page now documents them. Treat Reddit threads as community evidence: add a reference for a thread only when several reports agree, and never let a thread override a vendor page. Update `accessed` on every reference you re-read, and list anything you changed with the sentence from the page that justifies it.

## Rules the loader enforces

- A claim with `vendor` evidence must cite a `vendor` reference for that aspect, and a `community` claim must cite a `community` one. Quirks cite their own references.
- References need an https link and the date they were read.
- `cargo test -p boothready-core rules` fails on any gap, so run it after every edit.

When a claim can't be sourced, mark it `inferred`. The app then says "expected to work" for it, where a documented claim would say "ready".
