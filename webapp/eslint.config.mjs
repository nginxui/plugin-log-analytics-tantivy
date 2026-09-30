import createConfig from '@antfu/eslint-config'

export default createConfig(
  {
    stylistic: true,
    ignores: ['**/*.md', '**/world.json', '**/i18n/*.json', 'dist', 'tsconfig.json'],
  },
  {
    name: '@nginx-ui/plugin-eslint-config',
    rules: {
      'no-console': 'warn',
      'no-alert': 'warn',
      'ts/no-explicit-any': 'warn',
      'vue/no-unused-refs': 'warn',
      'vue/prop-name-casing': 'warn',
      'node/prefer-global/process': 'off',
      'unused-imports/no-unused-vars': 'warn',
      'import/no-duplicates': 'error',
      'no-undef': 'error',
      'style/dot-notation': 'off',
      'style/arrow-parens': ['error', 'as-needed'],
      'prefer-template': 'error',
      'style/arrow-spacing': ['error', { before: true, after: true }],
      'import/prefer-default-export': 'off',
      'vue/require-typed-ref': 'warn',
      'vue/require-prop-types': 'warn',
      'vue/no-ref-as-operand': 'error',
      'vue/component-name-in-template-casing': ['error', 'PascalCase', { registeredComponentsOnly: false }],
      'e18e/prefer-static-regex': 'off',
      'eslint-comments/no-unlimited-disable': 'off',
    },
  },
)
