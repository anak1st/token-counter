# 思考强度（thinking effort）专题分析

> 2026-10-09 调研 + 本机数据可行性实测。**结论：暂不实现**——本文档保留完整分析，未来要做时直接按 §5 的路径走，不必重新调研。

## 1. 外部视角：为什么有人关心这个指标

「思考强度」在学术界有正式名字：**overthinking（过度思考）**，2025–2026 是研究热点：

- **成本代价有量化**：OckBench 提出 "Overthinking Tax"——同准确率下过度思考的模型 token 消耗可达 5 倍，**实际更贵**；《The Danger of Overthinking》（arXiv 2502.08235）实测：压低 overthinking score 后任务成功率 **+30%、算力成本 −43%**
- **边际收益递减**：《More Thinking Hurts》（ACL 2026）发现思考越长边际收益越低、甚至推翻已正确的答案；cost-aware 评估显示**停在中等思考预算最省也最准**
- **工业界做法**：按任务调 `reasoningEffort` / thinking budget（简单任务直接关）、用 **think ratio（思考占生成 token 比）** 做监控；思考 token 按输出价计费，**同时推高账单和延迟**（研究口径：长推理单次 token 约为传统模型 8 倍）
- **中文社区**：2026 年关键词是「**推理性价比**」——过度思考既贵又慢，是性价比的直接敌人

**一个诚实的边界**：学界定义「过度」都需要**正确性标签**（答对了没有、是否必要）。本地数据没有正确性信息，所以即使实现，我们也只能报**强度**（思考占比 / 思考量），「是否过度」是使用侧的判断。

## 2. 本地数据可行性（实测）

### 2.1 reasoning 字段覆盖

| 源 | reasoning 精确值 | 记账语义 | effort 配置字段 |
|---|---|---|---|
| **mimo** | ✅ 独立字段；`total = input + output + reasoning + cache` 精确验证；241/249 条有值（合计 reasoning 65,694 vs output 87,671） | reasoning **不含**在 output 内 | `variant`（high 233 / null 24） |
| **dsh v4**（唯一保留的格式） | ❌ `usage` 无 `reasoningTokens`，只有思考**原文文本** | 未知（无真值） | `request/header.reasoningEffort`（high 35 / max 4） |
| dsh legacy（v3 / 旧格式，已静默放弃） | ✅ 有 `reasoningTokens` | reasoning **含**在 output 内（`total = input + output + cache` 验证） | 同上 |

**两源记账方式相反**（mimo 分离 / dsh legacy 包含）——统一 Think% 公式必须按源分支，本身就是一个口径风险。

### 2.2 原始输出能否区分思考与回答

**能（文本层），不能（token 层）**：

- dsh v4 的 `assistant/message.message.content` 存有完整原文，块类型分开：`reasoning`（思考）/ `text`（回答）/ `tool-call`（工具调用）——**哪段是思考精确可知**
- 但 usage 只有聚合的 `outputTokens`（三者混合），**没有按块的 token 计数**；换成 token 需要估算（见 §3）或引入 tokenizer（破坏零依赖）
- mimo 无此问题：`tokens.reasoning` 直接给出精确值

### 2.3 effort 配置字段

两源都有请求级的思考档位（mimo `variant`、dsh `reasoningEffort`，实测值 high/max），可作**弱替代信号**（反映配置，不反映实际消耗），成本为零。

## 3. 估算实验：文本长度 → token（如未来需要）

方法：用带 `reasoningTokens` 真值的历史格式消息做校准与回测（真值文件仍在磁盘上，只是不再参与统计）。

- 校准：约 **3.93 chars/token**（v3 文件 40 条真值拟合）
- 回测（**655 条**真值样本）：单条误差**中位数 7.5%**、p90 = 26.7%；总量偏差 −13.3%（中文思考 chars/token 更小导致偏低，比率修正到 **≈3.41** 可消除总量偏差）

**结论：日级聚合估算可用（校准后误差 <10%），单条级别不可用**；输出须标 `≈`。这是"如果非要 dsh Think%"时的方案——**当前决定不实现**。

## 4. 已验证的替代方向：每问题 effort 统计（分析完成，未实现）

比思考占比更直接的口径：**以「一问一答」为单位，看解决一个问题花了多少努力**——生成量、工具调用、步数、耗时、费用。全部是**精确值，零估算**，不依赖 reasoning 拆分。

可行性已实测验证：

- **dsh 回合边界**：`turn/start .. turn/end` 事件成对（含 aborted 标注）；问题文本取 `user/message` 且 `source.kind == "user"`（需过滤 `agent-instructions` / `runtime-context` 等注入）；`tool/call`、`step/start` 均带 `turn` 字段可计数
- **mimo 回合边界**：user 消息时间边界；**问题正文在 `part` 表的 text part 里**（`message.data` 没有正文）；步数/工具数数 `step-start` / `tool`+`patch` part
- **口径细节**：mimo 需按 `agent_id = 'main'` 过滤（库里有 checkpoint-writer 子 agent 的 33 条消息，不应混入"回答我的问题"的成本）
- **输出设计**：问题原文放**行尾**（中文数据不参与前面列的对齐，绕开"输出全英文"的排版约束）

预计实现量 ~250 行（adapter 挂回合元数据 + 聚合 + 渲染 + `--by turn` 参数）。**待定，等决定。**

## 5. 结论与未来实现路径

**当前决定（2026-10-09）：思考强度暂不实现。**

若未来要做：

1. **mimo 侧**：直接用 `tokens.reasoning`，精确，`Think% = reasoning ÷ (output + reasoning)`
2. **dsh v4 侧**：用 §3 校准比率估算（≈3.41 chars/token，标 `≈`），或等待上游恢复 reasoningTokens 字段
3. **零成本弱信号**：直接展示 `reasoningEffort` / `variant` 配置档位
4. 每问题 effort（§4）是与之互补的另一维度，两者不冲突

相关的记账真相备查：**mimo 的 reasoning 与 output 分离，dsh legacy 的 reasoning 包含在 output 内**——任何涉及 reasoning 的公式都必须记住这一点。
