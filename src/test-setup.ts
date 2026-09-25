/**
 * Vitest 全局测试环境准备。
 *
 * React 19 要求显式声明 act 环境，否则组件测试会打印
 * "The current testing environment is not configured to support act(...)"。
 */
(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT =
  true;

export {};
