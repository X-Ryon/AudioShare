import sys, asyncio
sys.stdout.reconfigure(encoding="utf-8")
from playwright.async_api import async_playwright

async def main():
    errors = []
    async with async_playwright() as p:
        browser = await p.chromium.launch(channel="msedge")
        page = await browser.new_page(viewport={"width": 500, "height": 820})
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.on("console", lambda m: errors.append(m.text) if m.type == "error" else None)
        await page.goto("file:///D:/WorkFiles/test/AudioRouter/产品库/迭代需求/2026-09-24-Windows11系统音频路由软件/文档/原型.html")
        await page.wait_for_timeout(600)

        checks = []

        def dims():
            return page.evaluate(
                "(() => { const el = document.getElementById('selPanel');"
                " return {sh: el.scrollHeight, ch: el.clientHeight}; })()")

        # 初始2行：无滚动
        d = await dims()
        checks.append(f"2行: scrollH={d['sh']} vs clientH={d['ch']} " + ("OK无滚动" if d['sh'] <= d['ch'] else "FAIL"))

        # 勾选5个应用 → 出现滚动
        await page.click("#appDdBtn")
        for app in ["chrome", "qqmusic", "steam"]:
            await page.click(f".dd-item[data-app={app}]")
        await page.click("#appDdBtn")
        rows = await page.locator("#selPanel .selrow").count()
        d = await dims()
        maxh = await page.evaluate("getComputedStyle(document.getElementById('selPanel')).maxHeight")
        checks.append(f"5行: 行数={rows}, maxH={maxh}, scrollH={d['sh']} > clientH={d['ch']} " + ("OK可滚动" if d['sh'] > d['ch'] else "FAIL"))

        # 滚动到底部第5行可见
        await page.evaluate("document.getElementById('selPanel').scrollTop = 99999")
        steam_vis = await page.locator(".selrow[data-sel=steam]").is_visible()
        checks.append("滚动后Steam行可见: " + ("OK" if steam_vis else "FAIL"))

        # 减回3行：不滚动（边界）
        await page.click("#appDdBtn")
        for app in ["qqmusic", "steam"]:
            await page.click(f".dd-item[data-app={app}]")
        await page.click("#appDdBtn")
        d = await dims()
        checks.append(f"3行: scrollH={d['sh']} vs clientH={d['ch']} " + ("OK边界不滚" if d['sh'] <= d['ch'] + 1 else "FAIL"))

        # 页面整体无横向溢出
        overflow = await page.evaluate("document.documentElement.scrollWidth > document.documentElement.clientWidth")
        checks.append("页面横向溢出: " + ("有(异常)" if overflow else "无"))

        print("\n".join(checks))
        print("JS错误数:", len(errors), errors[:3] if errors else "")
        await browser.close()

asyncio.run(main())
