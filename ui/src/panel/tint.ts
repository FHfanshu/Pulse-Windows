// Ported from upstream Panel/UsageTint.swift. Colour means usage, not brand.

export const PulseColor = {
  good: "rgb(0, 230, 140)",
  caution: "rgb(255, 194, 38)",
  warning: "rgb(255, 79, 66)",
  exhausted: "rgb(217, 23, 33)",
} as const;

export const CAUTION_THRESHOLD = 0.5;
export const DEFAULT_WARNING_THRESHOLD = 0.75;

export function usageColor(usedFraction: number, isExhausted: boolean, warningAt: number): string {
  if (isExhausted || usedFraction >= 1) return PulseColor.exhausted;
  if (usedFraction < CAUTION_THRESHOLD) return PulseColor.good;
  if (usedFraction < warningAt) return PulseColor.caution;
  return PulseColor.warning;
}

/** SwiftUI spring(response:dampingFraction:) as a motion spring (mass 1). */
export function spring(response: number, dampingFraction: number) {
  return {
    type: "spring" as const,
    stiffness: Math.pow((2 * Math.PI) / response, 2),
    damping: (4 * Math.PI * dampingFraction) / response,
    mass: 1,
  };
}
