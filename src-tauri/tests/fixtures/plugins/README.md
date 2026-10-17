# 插件宿主 supervisor 的测试夹具（Python，stdlib-only）。
#
# 每个文件都是「一个故意做错某件事的插件」，用来把 supervisor 的各条失败路径
# 变成可重复的断言。跑这些夹具需要 PATH 里有 `python`（本机 3.14）。
#
# 为什么不放在 plugins/examples 下：示例插件是给插件作者看的正面教材，
# 这些是故意崩、故意沉默的负面夹具，混在一起会让 SDK 阅读者困惑。
