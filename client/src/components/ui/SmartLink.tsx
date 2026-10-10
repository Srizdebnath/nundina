import Link from "next/link";
import type { ComponentProps } from "react";

type SmartLinkProps = Omit<ComponentProps<"a">, "href"> & { href: string; external?: boolean };

/** next/link for in-site routes, a plain new-tab anchor for absolute URLs. */
export function SmartLink({ href, external, children, ...rest }: SmartLinkProps) {
  if (external ?? /^https?:\/\//.test(href)) {
    return (
      <a href={href} target="_blank" rel="noopener noreferrer" {...rest}>
        {children}
      </a>
    );
  }
  return (
    <Link href={href} {...rest}>
      {children}
    </Link>
  );
}
