# 插件宿主 supervisor 的测试夹具

Python，stdlib-only。

每个文件都是「一个故意做错某件事的插件」，用来把 supervisor 的各条失败路径变成可重复的断言。跑这些夹具需要 PATH 里有 `python`（本机 3.14）。

| 文件                 | 故意做错的事                       | 被哪个断言盯住                       |
| -------------------- | ---------------------------------- | ------------------------------------ |
| `echo_plugin.py`     | 什么都不做错（对照组）             | 正常握手 + 方法调用                  |
| `silent_plugin.py`   | 启动后不读 stdin，永远不回帧       | 握手超时 10s 后被杀                  |
| `crashing_plugin.py` | 读到 `initialize` 后直接 `exit(1)` | 崩溃 → 结构化错误，宿主不 panic      |
| `chatty_stderr.py`   | 往 stderr 狂写，远超 64 KiB 环缓冲 | stderr 环缓冲只留尾部且不阻塞 stdout |

**为什么不放在 `plugins/examples` 下**：示例插件是给插件作者看的正面教材，这些是故意崩、故意沉默的负面夹具，混在一起会让 SDK 阅读者困惑。
