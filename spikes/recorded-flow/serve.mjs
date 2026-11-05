import { createServer } from "node:http";

const port = 18766;
const seconds = Number.isFinite(Number(process.argv[2])) ? Number(process.argv[2]) : 900;

const cards = Array.from({ length: 10 }, (_, index) => {
  const title = `動画 ${index + 1}`;
  return `<div class="card" data-card="${index + 1}" tabindex="0" role="button"><strong>${title}</strong><span>サンプル番組</span></div>`;
});

const html = `<!doctype html>
<html lang="ja">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>操作を覚えて集める デモ</title>
  <script src="https://cdn.jsdelivr.net/npm/hls.js@1.5.20/dist/hls.min.js"></script>
  <style>
    body { margin: 0; padding: 36px; background: #f5f4ef; color: #20201e; font: 15px/1.5 system-ui, sans-serif; }
    main { max-width: 1120px; margin: auto; }
    h1 { margin: 0 0 28px; font-size: 28px; }
    h2 { margin: 26px 0 12px; font-size: 18px; }
    .row { display: grid; grid-template-columns: repeat(5, minmax(0, 1fr)); gap: 14px; }
    .card { min-height: 118px; padding: 18px; border: 1px solid #cbc8bd; border-radius: 10px; background: #fff; cursor: pointer; box-sizing: border-box; }
    .card:hover, .card:focus-visible { border-color: #28724a; outline: 2px solid #28724a; outline-offset: 2px; }
    .card strong, .card span { display: block; }
    .card span { margin-top: 8px; color: #67645c; }
    dialog { width: min(620px, calc(100% - 48px)); padding: 0; border: 0; border-radius: 12px; box-shadow: 0 20px 70px #0006; }
    dialog::backdrop { background: #0008; }
    .modal { padding: 24px; }
    .modal-head { display: flex; align-items: center; justify-content: space-between; gap: 16px; }
    .modal h2 { margin: 0; }
    button { min-height: 36px; padding: 0 14px; border: 1px solid #aaa69a; border-radius: 7px; background: #fff; color: inherit; font: inherit; cursor: pointer; }
    .play { margin-top: 22px; border-color: #28724a; background: #28724a; color: #fff; }
    video { display: block; width: 100%; margin-top: 18px; background: #111; }
  </style>
</head>
<body>
  <main>
    <h1>番組を選ぶ</h1>
    <section><h2>新着</h2><div class="row">${cards.slice(0, 5).join("")}</div></section>
    <section><h2>人気</h2><div class="row">${cards.slice(5).join("")}</div></section>
  </main>
  <dialog><div class="modal"><div class="modal-head"><h2></h2><button class="close" type="button">閉じる</button></div><button class="play" type="button">再生</button><div class="player"></div></div></dialog>
  <script>
    const dialog = document.querySelector("dialog");
    const title = dialog.querySelector("h2");
    const player = dialog.querySelector(".player");
    let selected = 1;
    document.querySelectorAll(".card").forEach((card) => {
      const open = () => {
        selected = Number(card.dataset.card);
        title.textContent = card.querySelector("strong").textContent;
        player.replaceChildren();
        dialog.showModal();
      };
      card.addEventListener("click", open);
      card.addEventListener("keydown", (event) => { if (event.key === "Enter" || event.key === " ") open(); });
    });
    dialog.querySelector(".close").addEventListener("click", () => dialog.close());
    dialog.querySelector(".play").addEventListener("click", () => {
      const video = document.createElement("video");
      video.controls = true;
      video.muted = true;
      player.replaceChildren(video);
      const url = "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8?card=" + selected;
      if (window.Hls && Hls.isSupported()) {
        const hls = new Hls();
        hls.loadSource(url);
        hls.attachMedia(video);
        hls.on(Hls.Events.MANIFEST_PARSED, () => video.play().catch(() => {}));
      } else {
        video.src = url;
        video.play().catch(() => {});
      }
    });
  </script>
</body>
</html>`;

const server = createServer((request, response) => {
  const path = new URL(request.url ?? "/", `http://127.0.0.1:${port}`).pathname;
  if (path !== "/") {
    response.writeHead(404).end("Not found");
    return;
  }
  response.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" }).end(html);
});

server.listen(port, "127.0.0.1", () => {
  process.stdout.write(`Recorded-flow demo: http://127.0.0.1:${port}/\n`);
});

setTimeout(() => server.close(() => process.exit(0)), Math.max(0, seconds) * 1000);
