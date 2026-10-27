import { chromium } from '@playwright/test';
import { readFile } from 'node:fs/promises';
const image = (await readFile(new URL('../public/powerio-logo.svg', import.meta.url))).toString('base64');
const font = (await readFile(new URL('../node_modules/@fontsource-variable/atkinson-hyperlegible-next/files/atkinson-hyperlegible-next-latin-wght-normal.woff2', import.meta.url))).toString('base64');
const browser = await chromium.launch();
try {
  const page = await browser.newPage({ viewport: { width: 1200, height: 630 }, deviceScaleFactor: 1 });
  await page.setContent(`<!doctype html><style>
    @font-face{font-family:Atkinson;src:url(data:font/woff2;base64,${font})}*{box-sizing:border-box}
    body{margin:0;width:1200px;height:630px;background:#f7f9ff;color:#1f2c3d;font-family:Atkinson,sans-serif;padding:60px}
    header{font-size:29px;color:#4c617f}strong{color:#1f2c3d}h1{font-size:78px;line-height:1.03;letter-spacing:-2.5px;margin:70px 0 24px;font-weight:730}
    p{font-size:24px;color:#4c617f;line-height:1.4;margin:0}span{color:#285ded}
    .mascot{position:absolute;top:142px;left:813px;width:300px;transform:rotate(-6deg)}
    .ring{position:absolute;top:117px;left:777px;border:1px solid #d9e4fc;border-radius:50%;width:370px;height:370px}
    footer{position:absolute;bottom:48px;left:60px;right:60px;border-top:1px solid #d9e4fc;padding-top:20px;font-size:21px;color:#4c617f;display:flex;justify-content:space-between}
    </style><header><strong>PowerIO</strong> / Convert</header><h1>Convert power<br>system files<span>.</span></h1>
    <p>A file, a folder, or a whole batch.<br>Your files stay on your computer.</p>
    <div class="ring"></div><img class="mascot" src="data:image/svg+xml;base64,${image}" alt="">
    <footer><strong>powerio.dev/convert</strong><div>MATPOWER / PSS/E / OpenDSS / PMD / BMOPF</div></footer>`);
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: new URL('../public/social.png', import.meta.url).pathname });
} finally { await browser.close(); }
