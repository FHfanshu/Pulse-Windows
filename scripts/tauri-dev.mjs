// Windows difference: each worktree starts its own Vite instead of reusing another checkout's server.
import { createServer } from "node:net";
import { pathToFileURL } from "node:url";
import cli from "@tauri-apps/cli";

function available(port, host) {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", (error) => {
      if (error.code === "EADDRINUSE" || error.code === "EACCES") resolve(false);
      else if (host === "::1" && (error.code === "EADDRNOTAVAIL" || error.code === "EAFNOSUPPORT")) resolve(true);
      else reject(error);
    });
    server.listen({ port, host, ipv6Only: true }, () => server.close(() => resolve(true)));
  });
}

export async function selectDevPort() {
  for (let port = 1420; port < 1520; port++) {
    if (await available(port, "127.0.0.1") && await available(port, "::1")) return port;
  }
  throw new Error("No free development port between 1420 and 1519");
}

export function devConfig(port) {
  // Its own identity, so the single-instance check does not hand the launch to an installed Pulse
  // that is running: the two run side by side (the dev build also keeps its own data folder).
  return { identifier: "app.pulse.windows.dev", build: {
    devUrl: `http://127.0.0.1:${port}/panel.html`,
    // If another process takes the port after probing, fail instead of serving a different URL.
    beforeDevCommand: `npm run dev -- --host 127.0.0.1 --port ${port} --strictPort`,
  } };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const port = await selectDevPort();
    console.log(`Pulse dev: http://127.0.0.1:${port} (this checkout)`);
    await cli.run(["dev", "--config", JSON.stringify(devConfig(port)), ...process.argv.slice(2)], "npm run tauri:dev");
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
