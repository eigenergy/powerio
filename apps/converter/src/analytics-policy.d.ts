export {};
declare global {
  interface UsageEvent { name: string; data: Record<string, string> }
  var powerioAnalyticsPolicy: {
    normalize(name: unknown, data?: unknown): UsageEvent | null;
    safeFormat(value: unknown): string;
    safeCode(value: unknown): string;
    createBudget(): (event: UsageEvent) => boolean;
  };
}
