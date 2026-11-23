// 颜色输出：Chalk 统一封装，渲染层只依赖 Style 接口。
// --color always / never 显式控制；auto 交给 Chalk 的 TTY / NO_COLOR / FORCE_COLOR 探测。
// 颜色只用于结构性元素，数据值保持默认色；所有上色都发生在表格补齐之后。

import { Chalk } from "chalk";

export type ColorMode = "auto" | "always" | "never";

/** 渲染层使用的语义化样式；不直接 import chalk，便于测试与关色 */
export interface Style {
  /** 总计 / 组头：bold cyan */
  title(text: string): string;
  /** 表头与 Total 行：bold */
  header(text: string): string;
  /** Tokens 柱状图：cyan */
  bar(text: string): string;
  /** price 行等次要信息：dim */
  dim(text: string): string;
  /** 未知模型等提示：yellow */
  warning(text: string): string;
  /** 错误：red */
  error(text: string): string;
}

/** 无色样式：--color never 与测试使用 */
export const plainStyle: Style = {
  title: (text) => text,
  header: (text) => text,
  bar: (text) => text,
  dim: (text) => text,
  warning: (text) => text,
  error: (text) => text,
};

/** 解析颜色级别：always 固定 16 色；auto 使用 Chalk 的探测结果；never 关闭 */
function colorLevel(mode: ColorMode): 0 | 1 | 2 | 3 {
  if (mode === "never") return 0;
  if (mode === "always") return 1;
  return new Chalk().level;
}

/** 按 --color 模式创建样式；所有颜色方法都只返回字符串，不改变列宽 */
export function createStyle(mode: ColorMode): Style {
  const chalk = new Chalk({ level: colorLevel(mode) });
  return {
    title: (text) => chalk.bold.cyan(text),
    header: (text) => chalk.bold(text),
    bar: (text) => chalk.cyan(text),
    dim: (text) => chalk.dim(text),
    warning: (text) => chalk.yellow(text),
    error: (text) => chalk.red(text),
  };
}
