// Ported from upstream Panel/UsageTint.swift. Colour means usage, not brand.

export const PulseColor = {
  good: "rgb(0, 230, 140)",
  caution: "rgb(255, 194, 38)",
  warning: "rgb(255, 79, 66)",
  exhausted: "rgb(217, 23, 33)",
} as const;

/**
 * Deeper twins for the light panel (#74). The dark green and yellow are about 1.5:1 against a
 * light surface, which is no reading at all; these are 3:1 or better on `PanelLight.fill`.
 */
export const PulseColorLight = {
  good: "rgb(0, 148, 89)",
  caution: "rgb(194, 125, 0)",
  warning: "rgb(222, 56, 43)",
  exhausted: "rgb(173, 15, 26)",
} as const;

export const pulseColors = (light: boolean) => (light ? PulseColorLight : PulseColor);

export const CAUTION_THRESHOLD = 0.5;
export const DEFAULT_WARNING_THRESHOLD = 0.75;

export function usageColor(usedFraction: number, isExhausted: boolean, warningAt: number, light = false): string {
  const colors = pulseColors(light);
  if (isExhausted || usedFraction >= 1) return colors.exhausted;
  if (usedFraction < CAUTION_THRESHOLD) return colors.good;
  if (usedFraction < warningAt) return colors.caution;
  return colors.warning;
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
