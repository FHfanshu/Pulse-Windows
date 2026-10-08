// Ported from upstream Panel/UsageDockView.swift (DockLayout), Panel/UsageDetailCard.swift
// (DetailCardLayout), Usage/UsageSource.swift (PanelMetrics) and
// Panel/FloatingPanelController.swift (Layout). Names kept identical to upstream.

export type Axis = "vertical" | "horizontal";
export type Edge = "left" | "right" | "top" | "bottom";

export const axisOf = (edge: Edge): Axis => (edge === "left" || edge === "right" ? "vertical" : "horizontal");

export const PanelSizeScale = { small: 0.82, standard: 1, large: 1.22 } as const;
export const RailSpacingScale = { compact: 0.6, standard: 1, roomy: 1.4 } as const;

/** Upstream `PanelMetrics`: the layout budget every surface is measured from. */
export interface PanelMetrics {
  scale: number;
  spacing: number;
  topRailShowsPercentages: boolean;
  sideRailShowsPercentages: boolean;
  labelAboveRing: boolean;
  freeAcrossFiguresBeside: boolean;
  usesRoundEnds: boolean;
  showsWindowClock: boolean;
  showsForecast: boolean;
  showsDetailedCard: boolean;
  railCapacity: number;
}

export const defaultMetrics: PanelMetrics = {
  scale: 1,
  spacing: 1,
  topRailShowsPercentages: false,
  sideRailShowsPercentages: true,
  labelAboveRing: false,
  freeAcrossFiguresBeside: false,
  usesRoundEnds: false,
  showsWindowClock: false,
  showsForecast: false,
  showsDetailedCard: false,
  railCapacity: 1,
};

export function dockLayout(m: PanelMetrics) {
  const s = m.scale;
  const width = 64 * s;
  const ringDiameter = 36 * s;
  const ringLineWidth = 4 * s;
  const ringToTextSpacing = (m.showsWindowClock ? 11 : 6) * s;
  const percentFontSize = 13 * s;
  const percentTextHeight = 16 * s;
  const percentTextWidth = 38 * s;
  const itemSpacing = 30 * s * m.spacing;
  const horizontalPadding = 10 * s;
  const cornerRadius = m.usesRoundEnds ? width / 2 : 26 * s;
  const flareHeight = m.usesRoundEnds ? cornerRadius : 24 * s;
  const flareWidth = m.usesRoundEnds ? width - cornerRadius : 38 * s;
  const endRingOffset = m.usesRoundEnds ? 8 * s : 0;
  const verticalPadding = m.usesRoundEnds
    ? flareHeight + cornerRadius - ringDiameter / 2 + endRingOffset
    : 46 * s;
  const itemHeight = ringDiameter + ringToTextSpacing + percentTextHeight;

  const showsPercentages = (axis: Axis) =>
    axis === "vertical" ? m.sideRailShowsPercentages : m.topRailShowsPercentages;
  const labelLeads = m.labelAboveRing;
  const ringOffsetInItem = (axis: Axis) =>
    labelLeads && showsPercentages(axis) ? percentTextHeight + ringToTextSpacing : 0;
  const endPadding = (docked: boolean) => (docked ? verticalPadding : verticalPadding - flareHeight);
  const freeAcrossWithLabels = (axis: Axis, docked: boolean) =>
    axis === "horizontal" && !docked && showsPercentages("horizontal");
  const labelsBeside = (axis: Axis, docked: boolean) =>
    freeAcrossWithLabels(axis, docked) && m.freeAcrossFiguresBeside;
  const labelsStackedFree = (axis: Axis, docked: boolean) =>
    freeAcrossWithLabels(axis, docked) && !m.freeAcrossFiguresBeside;

  const itemLength = (axis: Axis, docked = true) => {
    if (!showsPercentages(axis)) return ringDiameter;
    if (labelsBeside(axis, docked)) return ringDiameter + ringToTextSpacing + percentTextWidth;
    return axis === "vertical" ? itemHeight : Math.max(ringDiameter, percentTextWidth);
  };
  const gap = (axis: Axis, docked = true) => {
    if (labelsBeside(axis, docked)) return 18 * s * m.spacing;
    if (labelsStackedFree(axis, docked)) return 20 * s * m.spacing;
    return itemSpacing;
  };
  const crossPadding = (axis: Axis, docked = true) =>
    labelsStackedFree(axis, docked) ? 14 * s : horizontalPadding;
  const thickness = (axis: Axis, docked = true) => {
    if (axis !== "horizontal" || !showsPercentages("horizontal") || labelsBeside(axis, docked)) return width;
    return itemHeight + crossPadding(axis, docked) * 2;
  };
  const length = (count: number, axis: Axis, docked = true) => {
    const n = Math.max(count, 1);
    return endPadding(docked) * 2 + itemLength(axis, docked) * n + gap(axis, docked) * (n - 1);
  };
  const size = (count: number, axis: Axis, docked = true) => {
    const along = length(count, axis, docked);
    const across = thickness(axis, docked);
    return axis === "vertical" ? { w: across, h: along } : { w: along, h: across };
  };
  const ringCentreAcross = (axis: Axis, docked = true) => {
    if (labelsBeside(axis, docked)) return thickness(axis, docked) / 2;
    const item = axis === "vertical" ? ringDiameter : showsPercentages(axis) ? itemHeight : ringDiameter;
    const lead = axis === "horizontal" ? ringOffsetInItem(axis) : 0;
    return (thickness(axis, docked) - item) / 2 + lead + ringDiameter / 2;
  };
  const firstRingAlong = (docked = true, axis: Axis = "vertical") => {
    const intoItem = labelsBeside(axis, docked)
      ? ringDiameter / 2
      : axis === "vertical"
        ? ringOffsetInItem(axis) + ringDiameter / 2
        : itemLength(axis) / 2;
    return endPadding(docked) + intoItem;
  };
  const ringStep = (axis: Axis, docked = true) => itemLength(axis, docked) + gap(axis, docked);
  const maximumLength = (axis: Axis, capacity = m.railCapacity) =>
    Math.max(length(capacity, axis, true), length(capacity, axis, false));

  const collapsedWidth = 6 * s;
  const collapsedHeight = 96 * s;
  const collapsedHitWidth = 20 * s;

  return {
    width,
    ringDiameter,
    ringLineWidth,
    ringToTextSpacing,
    percentFontSize,
    percentTextHeight,
    percentTextWidth,
    horizontalPadding,
    cornerRadius,
    flareHeight,
    flareWidth,
    secondRingDiameter: 26 * s,
    secondRingLineWidth: 2.5 * s,
    itemHeight,
    labelLeads,
    showsPercentages,
    endPadding,
    itemLength,
    gap,
    crossPadding,
    thickness,
    length,
    size,
    ringCentreAcross,
    firstRingAlong,
    ringStep,
    maximumLength,
    labelsBeside,
    collapsedWidth,
    collapsedHeight,
    collapsedHitWidth,
    collapsedSize: (axis: Axis) =>
      axis === "vertical" ? { w: collapsedWidth, h: collapsedHeight } : { w: collapsedHeight, h: collapsedWidth },
  };
}

export function detailCardLayout(m: PanelMetrics) {
  const s = m.scale;
  const dock = dockLayout(m);
  const padding = 18 * s;
  const contentSpacing = 14 * s;
  const rowInternalSpacing = 7 * s;
  const progressBarHeight = 6 * s;
  const headerHeight = 19 * s;
  const rowTextLineHeight = 14 * s;
  const footnoteHeight = 13 * s;
  const headerLineSpacing = 4 * s;
  const activitySpacing = 10 * s;
  const figureLabelHeight = 13 * s;
  const figureValueHeight = 17 * s;
  const unpricedFiguresHeight = figureLabelHeight + 2 * s + figureValueHeight;
  const figuresHeight = unpricedFiguresHeight + 2 * s + figureLabelHeight;
  const chartHeight = 30 * s;
  const promptCacheNameHeight = 2 * s + footnoteHeight;

  const rowHeight =
    rowTextLineHeight +
    rowInternalSpacing +
    progressBarHeight +
    rowInternalSpacing +
    rowTextLineHeight +
    (m.showsForecast ? rowInternalSpacing + rowTextLineHeight : 0) +
    (m.showsDetailedCard ? rowInternalSpacing + rowTextLineHeight : 0);

  const activityHeight =
    1 +
    activitySpacing + figureLabelHeight +
    activitySpacing + figuresHeight +
    activitySpacing + chartHeight +
    activitySpacing + rowTextLineHeight +
    activitySpacing + rowTextLineHeight +
    activitySpacing + rowTextLineHeight + promptCacheNameHeight +
    activitySpacing + footnoteHeight;

  const height = (count: number, footnote = false) =>
    padding * 2 +
    headerHeight +
    (m.showsDetailedCard ? headerLineSpacing + footnoteHeight : 0) +
    count * (contentSpacing + rowHeight) +
    (footnote ? contentSpacing + footnoteHeight : 0) +
    (m.showsDetailedCard ? contentSpacing + activityHeight : 0);

  return {
    width: 250 * s,
    padding,
    cornerRadius: (dock.ringDiameter + dock.ringLineWidth) / 2,
    pointerWidth: 20 * s,
    pointerHeight: 40 * s,
    horizontalGap: 8 * s,
    contentSpacing,
    rowInternalSpacing,
    progressBarHeight,
    headerHeight,
    titleFontSize: 14 * s,
    rowFontSize: 11.5 * s,
    messageFontSize: 12 * s,
    footnoteFontSize: 11 * s,
    headerIconSize: 16 * s,
    rowTextLineHeight,
    rowHeight,
    footnoteHeight,
    headerLineSpacing,
    activitySpacing,
    figureLabelHeight,
    figureValueHeight,
    figureFontSize: 14 * s,
    figuresHeight,
    unpricedFiguresHeight,
    chartHeight,
    activityHeight,
    height,
    estimatedHeight: height(2),
    maximumHeight: height(6, true),
  };
}

/** Upstream `FloatingPanelController.Layout.size`: rounded up to whole DIPs. */
export function panelSize(m: PanelMetrics, edge: Edge, docked = true) {
  const dock = dockLayout(m);
  const card = detailCardLayout(m);
  const reach = card.width + card.pointerWidth + card.horizontalGap;
  const raw =
    axisOf(edge) === "vertical"
      ? { w: reach + dock.thickness("vertical"), h: Math.max(dock.maximumLength("vertical"), card.maximumHeight) }
      : {
          w: Math.max(dock.maximumLength("horizontal"), card.width),
          h: dock.thickness("horizontal", docked) + card.horizontalGap + card.pointerWidth + card.maximumHeight,
        };
  return { w: Math.ceil(raw.w), h: Math.ceil(raw.h) };
}
