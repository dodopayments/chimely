import { cva, type VariantProps } from 'class-variance-authority';
import type { HTMLAttributes } from 'react';
import { cn } from '@/lib/utils';

const badgeVariants = cva(
  'inline-flex items-center rounded-md border px-2 py-0.5 text-xs font-medium whitespace-nowrap',
  {
    variants: {
      variant: {
        default: 'border-transparent bg-primary/10 text-primary',
        neutral: 'border-border bg-muted text-muted-foreground',
        outline: 'border-border text-foreground',
        // Semantic state badges — tinted background, state-colored text.
        success: 'border-transparent bg-success/15 text-success-foreground dark:text-success',
        warning: 'border-transparent bg-warning/20 text-warning-foreground dark:text-warning',
        warningHi:
          'border-transparent bg-warning-hi/15 text-warning-hi-foreground dark:text-warning-hi',
        danger: 'border-transparent bg-danger/15 text-danger',
      },
    },
    defaultVariants: { variant: 'default' },
  },
);

export interface BadgeProps
  extends HTMLAttributes<HTMLSpanElement>,
    VariantProps<typeof badgeVariants> {}

export function Badge({ className, variant, ...props }: BadgeProps) {
  return <span className={cn(badgeVariants({ variant }), className)} {...props} />;
}

export { badgeVariants };
