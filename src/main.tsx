import "@fontsource-variable/inter";
import "@fontsource-variable/jetbrains-mono";
import "@/styles/tokens.css";
import "@/styles/base.css";

import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "@/App";

// 窗口背景由 CSS 绘制；关掉浏览器自带的右键菜单，应用才像原生程序。
// 文本输入框保留自己的菜单。
document.addEventListener("contextmenu", (event) => {
  const target = event.target as HTMLElement;
  if (target.closest("input, textarea, [contenteditable='true'], .cm-content")) return;
  event.preventDefault();
});

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
