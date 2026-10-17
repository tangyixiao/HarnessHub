#!/usr/bin/env python3
"""故意永不回包：验证握手超时路径。

关键点：它必须**活着**且什么都不写。如果它立刻退出，宿主会走 EOF 分支
（stdout 已关闭），那就测不到超时了 —— 所以这里死等 stdin EOF。
"""

import sys

# 读掉 stdin 但一个字都不回。
for _line in sys.stdin:
    pass
