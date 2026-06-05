from __future__ import annotations


PRIMARY_SOURCE_ADAPTERS = [
    "arxiv",
    "huggingface_hub",
    "github_releases",
    "sec_edgar",
    "federal_register",
    "regulations_gov",
    "patents",
]

SECONDARY_DISCOVERY_ADAPTERS = [
    "searxng",
    "exa",
    "youtube_rss",
]

SOURCE_POLICY_PROMPT = """
Use direct fetches against primary sources whenever possible. Use SearXNG,
Exa, and Browse.sh as complementary research tools rather than substitutes for
one another.

Required discovery workflow:
- Use Hermes web_search for Exa semantic discovery. The project profile keeps
  web.search_backend set to exa so semantic search is not lost.
- Also run SearXNG through terminal/curl with AI_NEWS_SEARXNG_SEARCH_URL for
  broad meta-search URL and snippet diversity. The basic call is `curl -sG
  "$AI_NEWS_SEARXNG_SEARCH_URL" --data-urlencode format=json --data-urlencode
  "q=<query>"`, but SearXNG is configurable per request: choose categories,
  engines, language, time_range, and pageno parameters when they fit the
  research question. For AI/current-events research, consider multiple passes
  across general, news, science, scientific publications, it, repos, and social
  media categories. Inspect unresponsive_engines and adapt when an engine is
  rate-limited or blocked. Compare SearXNG candidates against Exa results
  instead of letting either source dominate.
- Use Hermes web_extract as the Exa-backed retrieval pass. The project profile
  sets web.extract_backend to exa, so web_extract should retrieve fuller page
  content from the strongest candidates and primary-source URLs.
- Use Browse.sh/browser automation when a promising source is JavaScript-heavy,
  dynamically rendered, search-gated, or hard to extract through plain HTTP.
  Prefer local Browse.sh CLI/browser sessions first; use Browserbase cloud only
  when it is configured and the task justifies it.

Treat SearXNG search results and Exa extractions as discovery/retrieval vectors,
not final evidence by themselves. Prefer primary sources for final claims and
artifacts. Plan adapters for arXiv API/RSS, Hugging Face Hub API, GitHub
releases/API, SEC EDGAR Atom/API, Federal Register API, Regulations.gov API, and
patent APIs. Treat YouTube RSS as a secondary or marketing-heavy signal. Do not
hard-code company blogs as global primary sources; discover official feeds and
persist reliable entity-attached sources.
""".strip()
