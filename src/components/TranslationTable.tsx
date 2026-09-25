/**
 * 兼容旧导入路径：`@/components/TranslationTable` 仍然导出 `TranslationTable`。
 *
 * 实现已经拆分到 `./translation-table/` 目录（容器 + EntryRow / FilterChips /
 * 工具栏 / 列表 / 运行生命周期 hook），这里只做转发，避免动 App 与测试的导入。
 */
export { TranslationTable } from "./translation-table/TranslationTable";
