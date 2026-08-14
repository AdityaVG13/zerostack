# Provider Cache Fact Sheet - 14 August 2026

This document is a dated operational input, not a timeless theorem. Provider policies may change and must be refreshed before public claims.

## OpenAI

Official prompt-caching documentation describes reusable exact prompt prefixes and GPT-5.6-and-later cache breakpoints. OpenAI's Prompt Caching 201 guidance recommends append-only conversation histories and avoiding changes to original prefixes in order to preserve exact-prefix matches.

Sources:

- https://developers.openai.com/api/docs/guides/prompt-caching
- https://developers.openai.com/cookbook/examples/prompt_caching_201

## Anthropic

Official Claude Platform documentation exposes standard five-minute and extended one-hour prompt-cache durations. Lifetime is measured from the start of a cache write/read request.

Source:

- https://docs.anthropic.com/en/docs/build-with-claude/prompt-caching

## xAI

Official xAI documentation reports cached-token observations and notes that cache misses occur on first use or after cache eviction. xAI's documentation also warns that routing and eviction mean hits are not guaranteed.

Sources:

- https://docs.x.ai/developers/advanced-api-usage/prompt-caching/usage-and-pricing
- https://docs.x.ai/llms.txt

## Google Gemini

Official Gemini documentation distinguishes implicit caching from explicit cached-content objects. The generateContent caching documentation allows an explicit TTL and currently states a one-hour default when a TTL is not provided.

Sources:

- https://ai.google.dev/gemini-api/docs/caching
- https://ai.google.dev/gemini-api/docs/generate-content/caching

## ZeroStack implication

Provider caches are L1 accelerators. ZeroStack L2 logical causal validity must not depend on provider residency. A provider miss may reprocess a compact decision view, but should not cause unchanged repository discovery while valid L2 state remains retained.
