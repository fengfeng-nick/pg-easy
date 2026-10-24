// 把应用图标源文件（1024px SVG）渲染成 PNG，再交给 `tauri icon` 生成各平台
// 尺寸：
//
//   node scripts/make-icon.mjs
//   npx tauri icon src-tauri/icons/source.png
//
// 画稿只维护 `source.svg` 这一份，脚本不再内嵌一份副本——以前两处各写一份，
// 改了一处就忘了另一处，图标和界面里的标记会不一样。
import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";

const source = "src-tauri/icons/source.svg";
const target = "src-tauri/icons/source.png";

readFileSync(source);
// `qlmanage` 在无窗口会话里会直接失败，`sips` 走的是同一条 ImageIO 渲染，
// 但不需要窗口服务。
execFileSync("sips", ["-s", "format", "png", source, "--out", target], { stdio: "ignore" });
execFileSync("sips", ["-z", "1024", "1024", target], { stdio: "ignore" });
