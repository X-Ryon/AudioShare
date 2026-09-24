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
        # 输出电平栏已移除
        outmeter = await page.locator(".outrow, #outBar").count()
        checks.append(f"输出电平栏已移除: {'OK' if outmeter == 0 else 'FAIL(仍存在)'}")

        # 关键元素
        for sel, name in [("#masterSwitch", "总开关"), ("#appDd", "下拉复选框"),
                          ("#micSelect", "麦克风下拉框"), ("#selPanel", "已共享栏位")]:
            cnt = await page.locator(sel).count()
            checks.append(f"{name}: {'OK' if cnt == 1 else 'MISSING'}")

        # 下拉项只显示应用名（无内联滑杆）
        dd_items = await page.locator(".dd-item").count()
        dd_vol = await page.locator(".dd-item input[type=range]").count()
        checks.append(f"下拉5个应用名、无内联滑杆: 项={dd_items}, 滑杆={dd_vol}({'OK' if dd_items == 5 and dd_vol == 0 else 'FAIL'})")

        # 初始已选栏位 = 网易云+抖音 两行，含电平条和滑杆
        rows = await page.locator("#selPanel .selrow").count()
        meters = await page.locator("#selPanel .srcmeter").count()
        vols = await page.locator("#selPanel input[type=range]").count()
        checks.append(f"已选栏位行数={rows}, 电平条={meters}, 滑杆={vols}({'OK' if rows==2 and meters==2 and vols==2 else 'FAIL'})")

        # 勾选QQ音�乐 → 栏位新增一行（AC-01）
        await page.click("#appDdBtn")
        await page.click(".dd-item[data-app=qqmusic]")
        rows2 = await page.locator("#selPanel .selrow").count()
        txt = await page.text_content("#appDdTxt")
        checks.append(f"勾选QQ音乐后: 行数={rows2}, 摘要='{txt}'({'OK' if rows2 == 3 and 'QQ音乐' in txt else 'FAIL'})")

        # 取消勾选网易云 → 行移除
        await page.click(".dd-item[data-app=cloudmusic]")
        rows3 = await page.locator("#selPanel .selrow").count()
        checks.append(f"取消网易云后行数={rows3}({'OK' if rows3 == 2 else 'FAIL'})")

        # 音量滑杆（AC-02）
        await page.locator(".selrow[data-sel=qqmusic] input[type=range]").fill("30")
        pct = await page.text_content(".selrow[data-sel=qqmusic] .pct")
        checks.append(f"QQ音乐音量调到30: {pct}({'OK' if pct == '30%' else 'FAIL'})")

        # 外部点击收起
        await page.click(".titlebar")
        panel_gone = not await page.locator("#appDd .dd-panel").is_visible()
        checks.append(f"点击外部收起下拉: {'OK' if panel_gone else 'FAIL'}")

        # 全部取消 → 空态
        await page.click("#appDdBtn")
        await page.click(".dd-item[data-app=douyin]")
        await page.click(".dd-item[data-app=qqmusic]")
        empty = await page.locator("#selPanel .selempty").count()
        checks.append(f"全部取消后空态提示: {'OK' if empty == 1 else 'FAIL'}")
        await page.click(".dd-item[data-app=douyin]")

        # 总开关（AC-03）
        await page.click("#appDdBtn") # 收起
        await page.click("#masterSwitch")
        aria = await page.get_attribute("#masterSwitch", "aria-checked")
        checks.append(f"总开关关闭: aria={aria}({'OK' if aria == 'false' else 'FAIL'})")
        await page.click("#masterSwitch")

        # 麦克风切换（AC-08）
        await page.select_option("#micSelect", "usb")
        val = await page.input_value("#micSelect")
        checks.append(f"切换麦克风: {val}({'OK' if val == 'usb' else 'FAIL'})")

        # 异常态：网易云退出 → 已选栏不含网易云（此时已选=抖音，网易云早已手动取消）
        await page.click("text=异常：网易云退出")
        rows4 = await page.locator("#selPanel .selrow").count()
        cm = await page.locator("#selPanel .selrow[data-sel=cloudmusic]").count()
        checks.append(f"网易云退出后: 行数={rows4}, 网易云行={cm}({'OK' if rows4 == 1 and cm == 0 else 'FAIL'})")
        await page.click("text=正常状态")
        await page.click("text=异常：虚拟声卡丢失")
        b1 = await page.is_visible("#devBanner")
        checks.append(f"虚拟声卡异常横幅: {'OK' if b1 else 'FAIL'}")
        await page.click("text=正常状态")

        # 溢出 + JS
        overflow = await page.evaluate("document.documentElement.scrollWidth > document.documentElement.clientWidth")
        checks.append(f"横向溢出: {'有(异常)' if overflow else '无'}")

        print("\n".join(checks))
        print("JS错误数:", len(errors), errors[:3] if errors else "")
        await browser.close()

asyncio.run(main())
