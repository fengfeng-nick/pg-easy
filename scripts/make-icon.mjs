// 用 macOS 的快速查看把应用图标源文件（1024px）渲染出来，然后交给
// `tauri icon` 生成各平台尺寸。
import { writeFileSync, renameSync } from "node:fs";
import { execFileSync } from "node:child_process";

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
  <defs>
    <linearGradient id="g" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#13807c"/>
      <stop offset="1" stop-color="#0a5656"/>
    </linearGradient>
    <linearGradient id="s" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#ffffff" stop-opacity="0.20"/>
      <stop offset="1" stop-color="#ffffff" stop-opacity="0"/>
    </linearGradient>
  </defs>
  <rect x="64" y="64" width="896" height="896" rx="200" fill="url(#g)"/>
  <rect x="64" y="64" width="896" height="448" rx="200" fill="url(#s)"/>
  <g fill="none" stroke="#ffffff" stroke-width="46" stroke-linecap="round">
    <ellipse cx="512" cy="318" rx="238" ry="88"/>
    <path d="M274 318v194c0 49 107 88 238 88s238-39 238-88V318"/>
    <path d="M274 512v194c0 49 107 88 238 88s238-39 238-88V512"/>
  </g>
  <circle cx="676" cy="690" r="30" fill="#ffffff" fill-opacity="0.9"/>
</svg>`;

writeFileSync("src-tauri/icons/source.svg", svg);
execFileSync("qlmanage", ["-t", "-s", "1024", "-o", "/tmp", "src-tauri/icons/source.svg"], {
  stdio: "ignore",
});
renameSync("/tmp/source.svg.png", "src-tauri/icons/source.png");
