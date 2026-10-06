import { describe, expect, it } from 'vitest';
import { displayPath } from './displayPath';

describe('displayPath', () => {
  it('removes a Windows local verbatim prefix', () => {
    expect(displayPath('\\\\?\\E:\\GaussianSplatting\\test\\002.mov')).toBe('E:\\GaussianSplatting\\test\\002.mov');
  });

  it('converts a Windows verbatim UNC path to a regular UNC path', () => {
    expect(displayPath('\\\\?\\UNC\\server\\share\\002.mov')).toBe('\\\\server\\share\\002.mov');
  });

  it('leaves ordinary paths unchanged', () => {
    expect(displayPath('E:\\Media\\002.mov')).toBe('E:\\Media\\002.mov');
    expect(displayPath('/mnt/media/002.mov')).toBe('/mnt/media/002.mov');
  });
});
