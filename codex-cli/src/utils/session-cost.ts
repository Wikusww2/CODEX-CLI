import type { ResponseItem } from "openai/resources/responses/responses.mjs";

import { approximateTokensUsed } from "./approximate-tokens-used.js";
import {
  estimateCostFromUsage,
  pricePerToken,
  type UsageBreakdown,
} from "./estimate-cost.js";

/**
 * Simple accumulator for {@link ResponseItem}s that exposes aggregate token
 * and (approximate) dollar‑cost statistics for the current conversation.
 */
export class SessionCostTracker {
  private readonly model: string;
  private readonly items: Array<ResponseItem> = [];

  private tokensUsedPrecise: number | null = null;

  /**
   * Aggregated exact cost when we have detailed `usage` information from the
   * OpenAI API.  Falls back to `null` when we only have the rough estimate
   * path available.
   */
  private costPrecise: number | null = null;

  constructor(model: string) {
    this.model = model;
  }

  /** Append newly‑received items to the internal history. */
  addItems(items: Array<ResponseItem>): void {
    this.items.push(...items);
  }

  /**
   * Add a full usage breakdown as returned by the Responses API.  This gives
   * us exact token counts and allows true‑to‑spec cost accounting that
   * factors in cached tokens.
   */
  addUsage(usage: UsageBreakdown): void {
    const tokens =
      usage.total_tokens ??
      (usage.input_tokens ?? 0) + (usage.output_tokens ?? 0);

    if (Number.isFinite(tokens) && tokens > 0) {
      this.tokensUsedPrecise = (this.tokensUsedPrecise ?? 0) + tokens;
    }

    const cost = estimateCostFromUsage(usage, this.model);
    if (cost != null) {
      this.costPrecise = (this.costPrecise ?? 0) + cost;
    }
  }

  /** Legacy helper for callers that only know the total token count. */
  addTokens(count: number): void {
    if (Number.isFinite(count) && count > 0) {
      this.tokensUsedPrecise = (this.tokensUsedPrecise ?? 0) + count;
      // We deliberately do *not* update costPrecise here – without a detailed
      // breakdown we cannot know whether tokens were input/output/cached.  We
      // therefore fall back to the blended rate during `getCostUSD()`.
    }
  }

  /** Approximate total token count so far. */
  getTokensUsed(): number {
    if (this.tokensUsedPrecise != null) {
      return this.tokensUsedPrecise;
    }
    return approximateTokensUsed(this.items);
  }

  /** Best‑effort USD cost estimate. Returns `null` when the model is unknown. */
  getCostUSD(): number | null {
    if (this.costPrecise != null) {
      return this.costPrecise;
    }

    const per = pricePerToken(this.model);
    if (per == null) {
      return null;
    }
    return this.getTokensUsed() * per;
  }

  /**
   * Human‑readable one‑liner suitable for printing at session end (e.g. on
   * Ctrl‑C or `/clear`).
   */
  summary(): string {
    const tokens = this.getTokensUsed();
    const cost = this.getCostUSD();
    if (cost == null) {
      return `Session complete – approx. ${tokens} tokens used.`;
    }
    return `Session complete – approx. ${tokens} tokens, $${cost.toFixed(
      4,
    )} USD.`;
  }
}

// ────────────────────────────────────────────────────────────────────────────
// Global helpers so disparate parts of the codebase can share a single
// tracker instance without threading it through countless function calls.
// ────────────────────────────────────────────────────────────────────────────

let globalTracker: SessionCostTracker | null = null;

export function getSessionTracker(): SessionCostTracker | null {
  return globalTracker;
}

export function ensureSessionTracker(model: string): SessionCostTracker {
  if (!globalTracker) {
    globalTracker = new SessionCostTracker(model);
  }
  return globalTracker;
}

export function resetSessionTracker(): void {
  globalTracker = null;
}

/**
 * Convenience helper that prints the session summary (if any) and resets the
 * global tracker so that the next conversation starts with a clean slate.
 */
export function printAndResetSessionSummary(): void {
  if (!globalTracker) {
    return; // nothing to do
  }

  // eslint-disable-next-line no-console -- explicit, user‑visible log
  console.log("\n" + globalTracker.summary() + "\n");

  resetSessionTracker();
}
