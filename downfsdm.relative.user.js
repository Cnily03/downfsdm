// ==UserScript==
// @name         FSDM Downloader Helper
// @author       Cnily03
// @license      Apache-2.0
// @icon         https://p0.ssl.qhimg.com/t016852bd7df93fb9de.png
// @version      0.1.0
// @description  Help download files from FSDM
// @namespace    https://github.com/Cnily03
// @downloadURL  https://raw.githubusercontent.com/Cnily03/downfsdm/refs/heads/main/downfsdm.relative.user.js
// @updateURL    https://raw.githubusercontent.com/Cnily03/downfsdm/refs/heads/main/downfsdm.relative.user.js
// @match        https://www.fsdm02.com/vodplay/*
// @grant        unsafeWindow
// ==/UserScript==

const BINARY = "./downfsdm";
const FFMPEG_BINARY = "./ffmpeg";

function requireDomLoaded(func) {
  return new Promise(resolve => {
    if (document.readyState !== 'loading') {
      func();
      resolve();
    } else {
      function temp() { func(); document.removeEventListener('DOMContentLoaded', temp); resolve(); }
      document.addEventListener('DOMContentLoaded', temp);
    }
  })
}

function appendDownloadCopy(code) {
  const opBox = document.querySelector(`.header-box .header-op`);
  const drop = document.createElement("div");
  drop.classList.add("drop");
  // topbar btn
  const downloadCopyBtn = document.createElement("div");
  downloadCopyBtn.classList.add("header-op-list-btn", "header-op-download-copy");
  downloadCopyBtn.innerHTML = `<i class="icon icon-download"></i><span>复制下载命令</span>`;
  // drop content
  const dropContent = document.createElement("div");
  dropContent.classList.add("drop-content", "drop-download-copy");
  const dropContentBox = document.createElement("div");
  dropContentBox.classList.add("drop-content-box");

  // drop inner
  const ul = document.createElement("ul");
  ul.classList.add("drop-content-items", "historical");

  const liTitle = document.createElement("li");
  liTitle.classList.add("drop-item", "drop-item-title", "no-after");
  liTitle.innerHTML = `<i class="icon icon-download"></i><strong>下载命令</strong>`;

  const style = document.createElement("style");
  style.textContent = `.no-after::after{display:none}.download-copy-code{white-space:break-spaces;word-break:break-all}`;
  document.head.appendChild(style);
  const liContent = document.createElement("li");
  liContent.classList.add("drop-item", "drop-item-content", "no-after");
  const codeEl = document.createElement("code");
  codeEl.classList.add("download-copy-code");
  codeEl.textContent = code;
  liContent.appendChild(codeEl);

  const liOp = document.createElement("li");
  liOp.classList.add("drop-item-op");
  const copyBtn = document.createElement("a");
  copyBtn.href = "javascript:void(0)";
  copyBtn.textContent = "复制";
  liOp.appendChild(copyBtn);

  // construct
  ul.appendChild(liTitle);
  ul.appendChild(liContent);
  ul.appendChild(liOp);

  dropContentBox.appendChild(ul);
  dropContent.appendChild(dropContentBox);
  drop.appendChild(downloadCopyBtn);
  drop.appendChild(dropContent);
  opBox.insertBefore(drop, opBox.firstChild);

  // listener
  let timeout = null;
  const changeCopyBtnText = (text) => {
    copyBtn.textContent = text;
    clearTimeout(timeout);
    timeout = setTimeout(() => {
      copyBtn.textContent = "复制";
    }, 1500);
  }
  copyBtn.addEventListener("click", () => {
    navigator.clipboard.writeText(code).then(() => {
      changeCopyBtnText("已复制");
    }).catch(err => {
      changeCopyBtnText("复制失败");
    });
  });
}

function collect() {
  const playerInfo = unsafeWindow.player_aaa || unsafeWindow.player_aaaa
  const vname = playerInfo.vod_data.vod_name;
  const url = `https://api.bytegooty.com/test16/?url=${encodeURIComponent(playerInfo.url)}&next=&name=${encodeURIComponent(vname)}`;
  return { vname, url };
}

function safeFilename(name) {
  return name.replace(/[\/\\?%*:|"<>]/g, '_');
}

function buildDownloadCommand({ vname, url }) {
  const argv = [
    "-o", JSON.stringify(`${safeFilename(vname)}.mp4`),
    "--ffmpeg-bin", JSON.stringify(FFMPEG_BINARY),
    "--cache-dir", JSON.stringify(`./${safeFilename(vname)}`),
    "--cleanup",
    JSON.stringify(url),
    '--',
    '-vcodec', 'copy',
    '-acodec', 'copy'
  ]
  return `${BINARY} ${argv.join(" ")}`;
}

!function () {
  'use strict';
  requireDomLoaded(() => {
    console.log("FSDM Downloader Helper loaded");
    const { vname, url } = collect();
    const code = buildDownloadCommand({ vname, url });
    console.log("Download command:", code);
    appendDownloadCopy(code);
  });
}();