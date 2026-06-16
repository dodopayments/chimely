import { Check, Copy, Eye, EyeOff } from 'lucide-react';
import { useState } from 'react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';

export function CopyField({
  value,
  maskable = false,
}: {
  value: string;
  maskable?: boolean;
}) {
  const [copied, setCopied] = useState(false);
  const [revealed, setRevealed] = useState(!maskable);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      toast.success('Copied to clipboard');
      setTimeout(() => setCopied(false), 1500);
    } catch {
      toast.error('Could not copy');
    }
  };

  return (
    <div className="flex items-center gap-2">
      <Input
        readOnly
        value={revealed ? value : '•'.repeat(Math.min(value.length, 32))}
        className="font-mono"
        onFocus={(e) => e.currentTarget.select()}
      />
      {maskable && (
        <Button variant="outline" size="icon" onClick={() => setRevealed((r) => !r)} type="button">
          {revealed ? <EyeOff /> : <Eye />}
        </Button>
      )}
      <Button variant="outline" size="icon" onClick={copy} type="button">
        {copied ? <Check className="text-success" /> : <Copy />}
      </Button>
    </div>
  );
}
