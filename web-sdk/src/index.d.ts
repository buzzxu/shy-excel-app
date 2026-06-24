/**
 * Type declarations for the ShyExcel client-export drop-in.
 * SOURCE: shy-excel-app/web-sdk/clientExport.js
 */

/** 建任务状态：建任务中 | 唤起中 | 已唤起 | 可能未安装。 */
export type ExportStatus = 'creating' | 'launching' | 'launched' | 'maybe_not_installed';

/** 最小 axios-like 接口：只需 get/post。 */
export interface AxiosLike {
  get(url: string, config?: { params?: Record<string, unknown> }): Promise<unknown>;
  post(url: string, data?: unknown): Promise<unknown>;
}

export interface LaunchClientExportOptions {
  /** 建任务接口路径，返回体含 {jobId, token, streamPath}（兼容被 Result 包一层）。 */
  jobApi: string;
  /** 导出筛选条件。 */
  params?: Record<string, unknown>;
  /** 建任务请求方法，默认 'GET'。 */
  method?: 'GET' | 'POST';
  /** axios-like 实例；传了则复用项目拦截器/鉴权/解包。 */
  request?: AxiosLike;
  /** 自定义 fetch（默认 window.fetch），仅在未传 request 时用。 */
  fetchImpl?: typeof fetch;
  /** fetch 路径下给 jobApi 拼的前缀（如 'https://api.example.com'）。 */
  baseURL?: string;
  /** fetch 路径下附加请求头。 */
  headers?: Record<string, string>;
  /**
   * 数据端点 origin（桌面客户端直连），默认 window.location.origin。
   * 若接口走 axios baseURL 前缀（如 /api），此处须显式传同样带前缀的 origin。
   */
  host?: string;
  /** 登录 token，作为 ?Authorization= 透传给客户端，默认 localStorage.token。 */
  auth?: string;
  /** 状态回调。 */
  onStatus?: (status: ExportStatus) => void;
  /** 未唤起时是否自动弹下载页，默认 true。 */
  autoPrompt?: boolean;
}

export interface LaunchClientExportResult {
  jobId: string;
  deepLink: string;
  streamUrl: string;
  launched: boolean;
}

export interface CreateJobResult {
  jobId: string;
  token: string;
  streamPath: string;
}

export const EXPORT_SCHEME: string;

export const CLIENT_DOWNLOAD: { windows: string; macos: string };

/** 唤起自定义协议并粗略判断是否成功（失焦=已唤起）。 */
export function launchProtocol(url: string, timeout?: number): Promise<boolean>;

/** 未检测到客户端时弹出「下载客户端」页。 */
export function showClientDownloadDialog(o?: { onRetry?: () => void }): void;

/** 双传输建任务（request 优先，否则 fetch）。 */
export function createJob(o: Omit<LaunchClientExportOptions, 'host' | 'onStatus' | 'autoPrompt'>): Promise<CreateJobResult>;

/** 由建任务结果拼出 streamUrl 与 deepLink。 */
export function buildDeepLink(o: { host?: string; streamPath?: string; jobId: string; token: string; auth?: string }): { streamUrl: string; deepLink: string };

/** 发起一次客户端高速导出。 */
export function launchClientExport(o: LaunchClientExportOptions): Promise<LaunchClientExportResult>;

export default launchClientExport;
