/** Amber "Coming soon" pill. Marks surfaces that are not shipped yet: the
 *  `npx chimely dev` CLI (hero inline command + quickstart). Amber #E5C07B is a
 *  neutral "pending" hue, deliberately not one of the brand state colors. */
export function ComingSoon() {
  return (
    <span className="inline-flex items-center rounded-full border border-[#E5C07B]/30 bg-[#E5C07B]/[0.12] px-2 py-0.5 font-mono text-[10px] font-semibold uppercase leading-none tracking-[0.07em] text-[#E5C07B]">
      Coming soon
    </span>
  );
}
