import { BuiltWithBar } from './sections/BuiltWithBar';
import { FinalCtaSection } from './sections/FinalCtaSection';
import { HeroSection } from './sections/HeroSection';
import { HowItsBuiltSection } from './sections/HowItsBuiltSection';
import { HowItWorksSection } from './sections/HowItWorksSection';
import { LandingFooter } from './sections/LandingFooter';
import { LandingNav } from './sections/LandingNav';
import { ProvablyFairSection } from './sections/ProvablyFairSection';
import { StatsBand } from './sections/StatsBand';
import { WhyDifferentSection } from './sections/WhyDifferentSection';

export function LandingPage() {
  return (
    <div id="top" className="min-h-full overflow-x-hidden bg-lp-bg text-white">
      <LandingNav />
      <main>
        <HeroSection />
        <BuiltWithBar />
        <HowItWorksSection />
        <WhyDifferentSection />
        <StatsBand />
        <ProvablyFairSection />
        <HowItsBuiltSection />
        <FinalCtaSection />
      </main>
      <LandingFooter />
    </div>
  );
}
