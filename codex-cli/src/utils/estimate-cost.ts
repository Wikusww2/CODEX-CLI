/**
 * Cost‑estimation helpers for OpenAI responses.
 *
 * The implementation now distinguishes between *input*, *cached input* and
 * *output* tokens, reflecting OpenAI’s 2025‑04 pricing scheme.  For models
 * where we only have a single blended rate we gracefully fall back to the
 * legacy logic so existing call‑sites continue to work.
 */

import type { ResponseItem } from "openai/resources/responses/responses.mjs";

import { approximateTokensUsed } from "./approximate-tokens-used.js";

// ────────────────────────────────────────────────────────────────────────────
// Pricing tables
// ────────────────────────────────────────────────────────────────────────────

/** Breakdown of per‑token prices (in USD). */
type TokenRates = {
  /** Price for *non‑cached* input prompt tokens. */
  input: number;
  /** Preferential price for *cached* input tokens. */
  cachedInput: number;
  /** Price for completion / output tokens. */
  output: number;
};

/**
 * Pricing table (exact model name -> per‑token rates).
 * All keys must be lower‑case.
 */
const detailedPriceMap: Record<string, TokenRates> = {
  // –––––––––––––– OpenAI “o‑series” experimental ––––––––––––––
  "o3": {
    input: 10 / 1_000_000,
    cachedInput: 2.5 / 1_000_000,
    output: 40 / 1_000_000,
  },
  "o4-mini": {
    input: 1.1 / 1_000_000,
    cachedInput: 0.275 / 1_000_000,
    output: 4.4 / 1_000_000,
  },

  // –––––––––––––– GPT‑4.1 family ––––––––––––––
  "gpt-4.1-nano": {
    input: 0.1 / 1_000_000,
    cachedInput: 0.025 / 1_000_000,
    output: 0.4 / 1_000_000,
  },
  "gpt-4.1-mini": {
    input: 0.4 / 1_000_000,
    cachedInput: 0.1 / 1_000_000,
    output: 1.6 / 1_000_000,
  },
  "gpt-4.1": {
    input: 2 / 1_000_000,
    cachedInput: 0.5 / 1_000_000,
    output: 8 / 1_000_000,
  },

  // –––––––––––––– GPT‑4o family ––––––––––––––
  "gpt-4o-mini": {
    input: 0.6 / 1_000_000,
    cachedInput: 0.3 / 1_000_000,
    output: 2.4 / 1_000_000,
  },
  "gpt-4o": {
    input: 5 / 1_000_000,
    cachedInput: 2.5 / 1_000_000,
    output: 20 / 1_000_000,
  },
};

/**
 * Legacy single‑rate pricing entries (per *thousand* tokens).  These are kept
 * to provide sensible fall‑backs for models that do not yet expose a detailed
 * breakdown or where we have no published split pricing.  The figures stem
 * from older OpenAI announcements and are only meant for *approximation* –
 * callers that rely on exact accounting should upgrade to models covered by
 * {@link detailedPriceMap}.
 */
const blendedPriceMap: Record<string, number> = {
  // GPT‑4 Turbo (Apr 2024)
  "gpt-4-turbo": 0.01,

  // Legacy GPT‑4 8k / 32k context models
  "gpt-4": 0.03,

  // GPT‑3.5‑Turbo family
  "gpt-3.5-turbo": 0.0005,

  // Remaining preview variants (exact names)
  "gpt-4o-search-preview": 0.0025,
  "gpt-4o-mini-search-preview": 0.00015,
  "gpt-4o-realtime-preview": 0.005,
  "gpt-4o-audio-preview": 0.0025,
  "gpt-4o-mini-audio-preview": 0.00015,
  "gpt-4o-mini-realtime-preview": 0.0006,
  "gpt-4o-mini": 0.00015,

  // Older experimental o‑series rates
  "o3-mini": 0.0011,
  "o1-mini": 0.0011,
  "o1-pro": 0.15,
  "o1": 0.015,

  // Additional internal preview models
  "computer-use-preview": 0.003,
};

// ────────────────────────────────────────────────────────────────────────────
// Public helpers
// ────────────────────────────────────────────────────────────────────────────

/**
 * Return the per‑token input/cached/output rates for the supplied model, or
 * `null` when no detailed pricing is available.
 */
function normalize(model: string): string {
  // Lower‑case and strip date/version suffixes like “‑2025‑04‑14”.
  const lower = model.toLowerCase();
  const dateSuffix = /-\d{4}-\d{2}-\d{2}$/;
  return lower.replace(dateSuffix, "");
}

export function priceRates(model: string): TokenRates | null {
  return detailedPriceMap[normalize(model)] ?? null;
}

/**
 * Fallback that returns a *single* blended per‑token rate when no detailed
 * split is available.  This mirrors the behaviour of the pre‑2025 version so
 * that existing callers keep working unmodified.
 */
export function pricePerToken(model: string): number | null {
  // Prefer an *average* of the detailed rates when we have them – this avoids
  // surprises where callers mix `pricePerToken()` with the new detailed
  // helpers.
  const rates = priceRates(model);
  if (rates) {
    return (rates.input + rates.output) / 2; // simple average heuristic
  }

  const entry = blendedPriceMap[normalize(model)];
  if (entry == null) {
    return null;
  }
  return entry / 1000;
}

// ────────────────────────────────────────────────────────────────────────────
// Cost estimation
// ────────────────────────────────────────────────────────────────────────────

/** Shape of the `usage` object returned by OpenAI’s Responses API. */
export type UsageBreakdown = {
  input_tokens?: number;
  input_tokens_details?: { cached_tokens?: number } | null;
  output_tokens?: number;
  total_tokens?: number;
};

/**
 * Calculate the exact cost (in USD) for a single usage breakdown.  Returns
 * `null` when the model is unknown.
 */
export function estimateCostFromUsage(
  usage: UsageBreakdown,
  model: string,
): number | null {
  const rates = priceRates(model);
  if (!rates) {
    // fall back to blended pricing
    const per = pricePerToken(model);
    if (per == null) {
      return null;
    }

    const tokens =
      usage.total_tokens ??
      (usage.input_tokens ?? 0) + (usage.output_tokens ?? 0);
    return tokens * per;
  }

  const input = usage.input_tokens ?? 0;
  const cached = usage.input_tokens_details?.cached_tokens ?? 0;
  const nonCachedInput = Math.max(0, input - cached);
  const output = usage.output_tokens ?? 0;

  return (
    nonCachedInput * rates.input +
    cached * rates.cachedInput +
    output * rates.output
  );
}

/**
 * Rough cost estimate (USD) for a series of {@link ResponseItem}s when using
 * the specified model.  When no detailed usage object is available we fall
 * back to estimating token counts based on the message contents.
 */
export function estimateCostUSD(
  items: Array<ResponseItem>,
  model: string,
): number | null {
  const per = pricePerToken(model);
  if (per == null) {
    return null;
  }
  return approximateTokensUsed(items) * per;
}
