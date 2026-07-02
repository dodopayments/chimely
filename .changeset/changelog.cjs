// Changelog lines without commit hashes. The default generator prefixes
// every entry with the releasing commit, which renders as noise on npm
// and points at the squash commit rather than the change.
const getReleaseLine = async (changeset) => {
  const [first, ...rest] = changeset.summary.trim().split('\n');
  return ['- ' + first, ...rest.map((line) => '  ' + line)].join('\n');
};

const getDependencyReleaseLine = async (_changesets, dependenciesUpdated) => {
  if (dependenciesUpdated.length === 0) {
    return '';
  }
  return [
    '- Updated dependencies:',
    ...dependenciesUpdated.map((dep) => `  - ${dep.name}@${dep.newVersion}`),
  ].join('\n');
};

module.exports = { getReleaseLine, getDependencyReleaseLine };
