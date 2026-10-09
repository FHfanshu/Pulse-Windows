// Getting a card out of the window: a file, a folder's worth of files, the clipboard (upstream
// RecapExport and RecapRenderer).
//
// The card is drawn again for each, at its full 1080 x 1920, never a screenshot of the scaled
// preview: it is mounted off screen at that size, serialised into an SVG `foreignObject`, and
// painted on a canvas. Everything about a card is inline style and inline SVG, so the serialised
// copy needs no stylesheet.
//
// Files go where the reader picks in a save (or folder) dialog and are written by the
// `save_file` command. "Share image" is not ported (there is no share sheet on Windows to anchor to).
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { createElement } from "react";
import { createRoot } from "react-dom/client";
import { CardView } from "./cards/CardView";
import { Deck, fileName } from "./deck";
import { CARD_HEIGHT, CARD_WIDTH } from "./kit";
import type { CardId } from "./types";

const frame = () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
const pause = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** The card as a 1080 x 1920 PNG. */
export async function renderPng(deck: Deck, card: CardId): Promise<Blob> {
  const stage = document.createElement("div");
  stage.style.cssText = `position:fixed;left:-${CARD_WIDTH + 200}px;top:0;width:${CARD_WIDTH}px;height:${CARD_HEIGHT}px;overflow:hidden;pointer-events:none;`;
  document.body.appendChild(stage);
  const root = createRoot(stage);
  try {
    root.render(createElement(CardView, { deck, card }));
    // Layout effects (fitted text, measured charts) settle over a few frames.
    await frame();
    await frame();
    await (document as Document & { fonts?: FontFaceSet }).fonts?.ready;
    await pause(60);
    await frame();

    const node = stage.firstElementChild as HTMLElement | null;
    if (!node) throw new Error("the card did not render");
    const clone = node.cloneNode(true) as HTMLElement;
    clone.setAttribute("xmlns", "http://www.w3.org/1999/xhtml");
    const xml = new XMLSerializer().serializeToString(clone);
    const svg =
      `<svg xmlns="http://www.w3.org/2000/svg" width="${CARD_WIDTH}" height="${CARD_HEIGHT}" viewBox="0 0 ${CARD_WIDTH} ${CARD_HEIGHT}">` +
      `<foreignObject x="0" y="0" width="${CARD_WIDTH}" height="${CARD_HEIGHT}">${xml}</foreignObject></svg>`;

    const image = new Image();
    image.decoding = "sync";
    image.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(svg);
    await image.decode();

    const canvas = document.createElement("canvas");
    canvas.width = CARD_WIDTH;
    canvas.height = CARD_HEIGHT;
    const context = canvas.getContext("2d");
    if (!context) throw new Error("no canvas");
    context.fillStyle = "#F5F5F1";
    context.fillRect(0, 0, CARD_WIDTH, CARD_HEIGHT);
    context.drawImage(image, 0, 0, CARD_WIDTH, CARD_HEIGHT);
    return await new Promise<Blob>((resolve, reject) => canvas.toBlob((blob) => (blob ? resolve(blob) : reject(new Error("no image"))), "image/png"));
  } finally {
    root.unmount();
    stage.remove();
  }
}

async function write(blob: Blob, path: string) {
  const bytes = Array.from(new Uint8Array(await blob.arrayBuffer()));
  await invoke("save_file", { path, bytes });
}

/** "cancelled": the reader closed the dialog; nothing is said. */
export type Outcome = "saved" | "copied" | "failed" | "cancelled";

/** The card on screen to a PNG file, `pulse-recap-2026-09-02-opener.png`. */
export async function saveCard(deck: Deck, card: CardId): Promise<Outcome> {
  try {
    const path = await save({ defaultPath: fileName(card, deck), filters: [{ name: "PNG", extensions: ["png"] }] });
    if (!path) return "cancelled";
    await write(await renderPng(deck, card), path.toLowerCase().endsWith(".png") ? path : `${path}.png`);
    return "saved";
  } catch {
    return "failed";
  }
}

/** Every card of the deck, poster included, in deck order, into a folder the reader picks. A file of
 *  the same name is replaced, nothing else is touched. */
export async function saveAll(deck: Deck): Promise<Outcome> {
  try {
    const folder = await open({ directory: true, multiple: false });
    if (!folder || Array.isArray(folder)) return "cancelled";
    for (const card of deck.cards) {
      await write(await renderPng(deck, card), `${folder.replace(/[\\/]+$/, "")}\\${fileName(card, deck)}`);
      // Let the window draw between cards.
      await pause(150);
    }
    return "saved";
  } catch {
    return "failed";
  }
}

/** The card as a PNG on the clipboard. */
export async function copyCard(deck: Deck, card: CardId): Promise<Outcome> {
  try {
    const blob = await renderPng(deck, card);
    await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
    return "copied";
  } catch {
    return "failed";
  }
}
