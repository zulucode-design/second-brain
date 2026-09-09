import type { AiProvider } from '$lib/types';

export type AiKeySlot = 'anthropic' | 'openai' | 'ollama' | 'openaiCompatible';
export type AiServerKind = 'ollama' | 'openaiCompatible' | null;
export type AiModelInput = 'anthropicChoices' | 'openaiChoices' | 'custom';

export interface AiProviderMetadata {
	label: string;
	defaultModel: string;
	keySlot: AiKeySlot;
	serverKind: AiServerKind;
	modelInput: AiModelInput;
	requiresApiKey: boolean;
	hasConfigurableAddress: boolean;
	canProbe: boolean;
	remoteService: string | null;
	keyPlaceholder: string;
	keyHelpUrl: string | null;
	keyHelpLabel: string | null;
}

/** Adding an AiProvider is a type error until its complete settings behavior is declared. */
export const AI_PROVIDER_METADATA: Record<AiProvider, AiProviderMetadata> = {
	ollama: {
		label: 'Ollama', defaultModel: 'gemma3:4b', keySlot: 'ollama', serverKind: 'ollama',
		modelInput: 'custom', requiresApiKey: false, hasConfigurableAddress: true, canProbe: true,
		remoteService: null, keyPlaceholder: 'Only required if your server requires auth', keyHelpUrl: null, keyHelpLabel: null,
	},
	anthropic: {
		label: 'Anthropic', defaultModel: 'claude-sonnet-4-6', keySlot: 'anthropic', serverKind: null,
		modelInput: 'anthropicChoices', requiresApiKey: true, hasConfigurableAddress: false, canProbe: false,
		remoteService: 'Anthropic', keyPlaceholder: 'sk-ant-...', keyHelpUrl: 'https://console.anthropic.com/settings/keys', keyHelpLabel: 'console.anthropic.com',
	},
	openai: {
		label: 'OpenAI', defaultModel: 'gpt-5.5', keySlot: 'openai', serverKind: null,
		modelInput: 'openaiChoices', requiresApiKey: true, hasConfigurableAddress: false, canProbe: false,
		remoteService: 'OpenAI', keyPlaceholder: 'sk-...', keyHelpUrl: 'https://platform.openai.com/api-keys', keyHelpLabel: 'platform.openai.com',
	},
	openai_compatible: {
		label: 'OpenAI Compatible', defaultModel: '', keySlot: 'openaiCompatible', serverKind: 'openaiCompatible',
		modelInput: 'custom', requiresApiKey: false, hasConfigurableAddress: true, canProbe: true,
		remoteService: null, keyPlaceholder: 'Leave empty if no auth required', keyHelpUrl: null, keyHelpLabel: null,
	},
};

export const AI_PROVIDER_OPTIONS = Object.entries(AI_PROVIDER_METADATA) as Array<
	[AiProvider, AiProviderMetadata]
>;
