import { cn } from "@/lib/utils";
import { ENVIRONMENT_LABELS, type ConnectionEnvironment } from "@/types/connection";

/** A connection's environment, short: PROD stands out, the others stay quiet. */
export function EnvironmentTag({
  environment,
  className,
}: {
  environment: ConnectionEnvironment | null | undefined;
  className?: string;
}) {
  if (!environment) return null;
  const { name, short } = ENVIRONMENT_LABELS[environment];
  return (
    <span
      title={name}
      className={cn(
        "inline-flex h-3.5 shrink-0 items-center rounded-[3px] border px-1 font-mono text-[9.5px] leading-none font-semibold tracking-wide",
        environment === "production"
          ? "border-danger/45 bg-danger/12 text-danger"
          : "border-line text-fg-3",
        className,
      )}
    >
      {short}
    </span>
  );
}
